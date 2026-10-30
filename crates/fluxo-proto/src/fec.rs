//! FEC (correção de erros sem retransmissão) por paridade XOR.
//!
//! Os fragmentos de um frame são divididos em grupos de `K`. Para cada grupo
//! sai um pacote extra de **paridade**: o XOR de todos os payloads do grupo.
//! Se exatamente **um** fragmento do grupo se perder, o receptor o
//! reconstrói fazendo o XOR da paridade com os que chegaram — sem pedir nada
//! ao host e sem esperar uma ida e volta.
//!
//! ```text
//! grupo:     A        B        C            paridade = A ⊕ B ⊕ C
//! perdeu B:  A   ?   C   P    →   B = P ⊕ A ⊕ C
//! ```
//!
//! ## Tamanhos diferentes
//!
//! O último fragmento de um frame costuma ser menor. Para o XOR, payloads
//! curtos contam como completados com zeros até o maior do grupo, e a
//! paridade carrega também o XOR dos **comprimentos** (2 bytes no início),
//! para o receptor saber onde cortar o fragmento reconstruído.
//!
//! Formato do payload de paridade: `[len_xor: u16 big-endian][xor dos dados]`.
//!
//! ## Custo e alcance
//!
//! Com `K = 10`: 10% a mais de pacotes. Um grupo só se perde se perder **2
//! ou mais** pacotes (contando a própria paridade). Com 1% de perda, a
//! chance disso é ~0,5% por grupo, contra ~10% de perder algum pacote sem
//! FEC.

/// Bytes de cabeçalho dentro do payload de paridade (o XOR dos comprimentos).
pub const PARITY_HEADER_LEN: usize = 2;

/// Monta o payload de paridade de um grupo.
///
/// ```
/// use fluxo_proto::fec::{parity, recover};
///
/// let grupo: [&[u8]; 3] = [b"abc", b"defg", b"hi"];
/// let p = parity(&grupo);
/// // Perdeu o do meio: reconstrói com a paridade e os outros dois.
/// assert_eq!(recover(&p, &[b"abc", b"hi"]).unwrap(), b"defg");
/// ```
pub fn parity(payloads: &[&[u8]]) -> Vec<u8> {
    let maior = payloads.iter().map(|p| p.len()).max().unwrap_or(0);
    let mut out = vec![0u8; PARITY_HEADER_LEN + maior];
    let mut len_xor = 0u16;
    for p in payloads {
        len_xor ^= p.len() as u16;
        for (o, b) in out[PARITY_HEADER_LEN..].iter_mut().zip(p.iter()) {
            *o ^= b;
        }
    }
    out[..PARITY_HEADER_LEN].copy_from_slice(&len_xor.to_be_bytes());
    out
}

/// Reconstrói o único payload faltante de um grupo, dados a paridade e
/// **todos** os outros payloads do grupo.
///
/// `None` se a paridade for incoerente (curta demais ou comprimento
/// impossível) — pacote corrompido ou forjado.
pub fn recover(parity: &[u8], others: &[&[u8]]) -> Option<Vec<u8>> {
    if parity.len() < PARITY_HEADER_LEN {
        return None;
    }
    let mut len = u16::from_be_bytes([parity[0], parity[1]]);
    let mut data = parity[PARITY_HEADER_LEN..].to_vec();
    for p in others {
        len ^= p.len() as u16;
        if p.len() > data.len() {
            return None; // a paridade cobre sempre o maior payload do grupo
        }
        for (d, b) in data.iter_mut().zip(p.iter()) {
            *d ^= b;
        }
    }
    let len = len as usize;
    if len > data.len() {
        return None;
    }
    data.truncate(len);
    Some(data)
}

/// Quantos grupos de `group` fragmentos cobrem `frag_count` fragmentos.
pub fn group_count(frag_count: u16, group: u8) -> u16 {
    match group {
        0 => 0,
        g => frag_count.div_ceil(g as u16),
    }
}

/// Faixa de índices de fragmento coberta pelo grupo `g`.
pub fn group_range(g: u16, group: u8, frag_count: u16) -> std::ops::Range<u16> {
    let inicio = g.saturating_mul(group as u16);
    inicio.min(frag_count)..inicio.saturating_add(group as u16).min(frag_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstroi_qualquer_um_que_falte() {
        let grupo: Vec<Vec<u8>> = (0..10u8).map(|i| vec![i.wrapping_mul(37); 1200]).collect();
        let mut ultimo = grupo.clone();
        ultimo[9].truncate(333); // último fragmento do frame, menor
        let refs: Vec<&[u8]> = ultimo.iter().map(Vec::as_slice).collect();
        let p = parity(&refs);
        assert_eq!(p.len(), PARITY_HEADER_LEN + 1200);
        for (faltou, esperado) in ultimo.iter().enumerate() {
            let outros: Vec<&[u8]> = refs
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != faltou)
                .map(|(_, r)| *r)
                .collect();
            assert_eq!(recover(&p, &outros).unwrap(), *esperado, "faltou {faltou}");
        }
    }

    #[test]
    fn grupo_de_um_e_uma_copia() {
        let p = parity(&[b"sozinho"]);
        assert_eq!(recover(&p, &[]).unwrap(), b"sozinho");
    }

    #[test]
    fn paridade_corrompida_e_recusada() {
        assert_eq!(recover(&[0], &[]), None);
        // Comprimento declarado maior que os dados.
        assert_eq!(recover(&[0, 9, 1, 2], &[]), None);
        // Um "outro" maior que a paridade não pode ser do mesmo grupo.
        assert_eq!(recover(&[0, 1, 7], &[b"longo demais"]), None);
    }

    #[test]
    fn grupos_cobrem_todos_os_fragmentos() {
        assert_eq!(group_count(25, 10), 3);
        assert_eq!(group_range(0, 10, 25), 0..10);
        assert_eq!(group_range(2, 10, 25), 20..25);
        assert_eq!(group_count(10, 10), 1);
        assert_eq!(group_count(1, 10), 1);
        assert_eq!(group_count(7, 0), 0);
    }
}
