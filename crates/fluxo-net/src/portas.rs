//! Os sockets de uma ponta: [`Portas`] (IPv4 e, se houver, IPv6).
//!
//! Antes do pareamento o outro lado pode responder por qualquer família,
//! então escutamos nas duas; depois fica só o socket da família que pareou
//! ([`Portas::keep_only`]), e o vídeo não paga nada pela outra.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use crate::is_transient;

/// Buffers de envio e recepção de cada socket. O padrão do Windows (64 KB)
/// não segura a rajada de um keyframe em 4K: a 100 Mbps na mesma máquina, 5
/// frames em 600 se perdiam no buffer de recepção.
const BUFFER_SOCKET: i32 = 4 << 20;

/// Pede [`BUFFER_SOCKET`] para envio e recepção. O sistema pode dar menos
/// (no Linux, o teto é `net.core.rmem_max`); falha é ignorada.
fn ampliar_buffers(s: &UdpSocket) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        #[link(name = "ws2_32")]
        extern "system" {
            fn setsockopt(s: usize, level: i32, name: i32, val: *const i32, len: i32) -> i32;
        }
        const SOL_SOCKET: i32 = 0xffff;
        for opcao in [0x1001, 0x1002] {
            // SO_SNDBUF, SO_RCVBUF
            // SAFETY: socket válido (emprestado), ponteiro para um i32 vivo.
            unsafe { setsockopt(s.as_raw_socket() as usize, SOL_SOCKET, opcao, &BUFFER_SOCKET, 4) };
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn setsockopt(fd: i32, level: i32, name: i32, val: *const i32, len: u32) -> i32;
        }
        // SOL_SOCKET, SO_SNDBUF, SO_RCVBUF: valores do Linux; os BSD (macOS) usam os do Windows.
        let (nivel, opcoes) = match cfg!(target_os = "linux") {
            true => (1, [7, 8]),
            false => (0xffff, [0x1001, 0x1002]),
        };
        for opcao in opcoes {
            // SAFETY: socket válido (emprestado), ponteiro para um i32 vivo.
            unsafe { setsockopt(s.as_raw_fd(), nivel, opcao, &BUFFER_SOCKET, 4) };
        }
    }
}

pub(crate) struct Portas {
    v4: Option<UdpSocket>,
    v6: Option<UdpSocket>,
}

impl Portas {
    pub(crate) fn new(v4: Option<UdpSocket>, v6: Option<UdpSocket>) -> io::Result<Self> {
        if v4.is_none() && v6.is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "nenhum socket"));
        }
        for s in v4.iter().chain(v6.iter()) {
            ampliar_buffers(s);
        }
        Ok(Self { v4, v6 })
    }

    /// Um socket só, na família dele.
    pub(crate) fn one(s: UdpSocket) -> io::Result<Self> {
        match s.local_addr()? {
            SocketAddr::V4(_) => Self::new(Some(s), None),
            SocketAddr::V6(_) => Self::new(None, Some(s)),
        }
    }

    /// O socket da família de `peer`, para uso exclusivo (o outro é fechado).
    pub(crate) fn into_socket(mut self, peer: SocketAddr) -> io::Result<UdpSocket> {
        match peer {
            SocketAddr::V4(_) => self.v4.take(),
            SocketAddr::V6(_) => self.v6.take(),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "sem socket dessa família"))
    }

    fn para(&self, destino: &SocketAddr) -> io::Result<&UdpSocket> {
        match destino {
            SocketAddr::V4(_) => self.v4.as_ref(),
            SocketAddr::V6(_) => self.v6.as_ref(),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "sem socket dessa família"))
    }

    pub(crate) fn send_to(&self, buf: &[u8], destino: SocketAddr) -> io::Result<usize> {
        self.para(&destino)?.send_to(buf, destino)
    }

    /// Lê do primeiro socket que tiver algo. Com os dois, cada um espera
    /// metade do timeout (ver [`set_read_timeout`](Self::set_read_timeout)).
    pub(crate) fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        match (&self.v4, &self.v6) {
            (Some(a), Some(b)) => match a.recv_from(buf) {
                Err(e) if is_transient(&e) => b.recv_from(buf),
                r => r,
            },
            (Some(s), None) | (None, Some(s)) => s.recv_from(buf),
            (None, None) => unreachable!("conferido em new"),
        }
    }

    pub(crate) fn set_read_timeout(&self, t: Duration) -> io::Result<()> {
        let cada = match self.v4.is_some() && self.v6.is_some() {
            true => (t / 2).max(Duration::from_millis(1)),
            false => t,
        };
        for s in self.v4.iter().chain(&self.v6) {
            s.set_read_timeout(Some(cada))?;
        }
        Ok(())
    }

    /// Fica só com o socket da família de `peer` (o outro é fechado) e o
    /// devolve para uso direto.
    pub(crate) fn keep_only(&mut self, peer: SocketAddr) -> io::Result<&UdpSocket> {
        match peer {
            SocketAddr::V4(_) => self.v6 = None,
            SocketAddr::V6(_) => self.v4 = None,
        }
        self.para(&peer)
    }

    /// O socket IPv4, se houver (TTL, endereços locais).
    pub(crate) fn v4(&self) -> Option<&UdpSocket> {
        self.v4.as_ref()
    }

    pub(crate) fn has_v6(&self) -> bool {
        self.v6.is_some()
    }

    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        match (&self.v4, &self.v6) {
            (Some(s), _) | (None, Some(s)) => s.local_addr(),
            (None, None) => unreachable!("conferido em new"),
        }
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            v4: self.v4.as_ref().map(UdpSocket::try_clone).transpose()?,
            v6: self.v6.as_ref().map(UdpSocket::try_clone).transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escuta_nas_duas_familias_e_depois_em_uma() {
        let v6 = match UdpSocket::bind("[::1]:0") {
            Ok(s) => s,
            Err(_) => return, // máquina sem IPv6: nada a testar
        };
        let v4 = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (a4, a6) = (v4.local_addr().unwrap(), v6.local_addr().unwrap());
        let mut p = Portas::new(Some(v4), Some(v6)).unwrap();
        p.set_read_timeout(Duration::from_millis(40)).unwrap();
        let fora4 = UdpSocket::bind("127.0.0.1:0").unwrap();
        let fora6 = UdpSocket::bind("[::1]:0").unwrap();
        let mut buf = [0u8; 8];
        fora6.send_to(b"seis", a6).unwrap();
        let (n, de) = p.recv_from(&mut buf).unwrap();
        assert_eq!((&buf[..n], de), (&b"seis"[..], fora6.local_addr().unwrap()));
        fora4.send_to(b"quatro", a4).unwrap();
        let (n, _) = p.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"quatro");
        // Cada destino sai pelo socket da sua família.
        p.send_to(b"oi", fora6.local_addr().unwrap()).unwrap();
        assert_eq!(fora6.recv_from(&mut buf).unwrap().1, a6);

        p.keep_only(fora4.local_addr().unwrap()).unwrap();
        assert!(!p.has_v6());
        assert!(p.send_to(b"x", fora6.local_addr().unwrap()).is_err());
    }
}
