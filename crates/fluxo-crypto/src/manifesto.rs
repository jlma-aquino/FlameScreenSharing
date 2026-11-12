//! Manifesto assinado de uma versão: [`UpdateManifest`].
//!
//! Cada versão publicada vai com um `MANIFESTO.txt` que diz qual é a versão,
//! qual é o arquivo e o SHA-256 dele, assinado (Ed25519) com a chave privada
//! de quem publica, que nunca sai do PC dele. O programa traz só a chave
//! pública e recusa atualização sem assinatura que confira. Como a versão vai
//! dentro do que é assinado, não dá para servir um zip antigo (e assinado)
//! como se fosse novo: o atualizador também exige versão maior que a sua.
//!
//! ```text
//! FlameScreenSharing atualizacao v1
//! versao = 0.2.0
//! arquivo = FlameScreenSharing.zip
//! sha256 = 3f9a…
//! assinatura = 9c1e…
//! ```

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::CryptoError;

/// Primeira linha do manifesto (e separador de domínio da assinatura).
const CABECALHO: &str = "FlameScreenSharing atualizacao v1";

/// Bytes da chave pública Ed25519.
pub const UPDATE_PUBLIC_KEY_LEN: usize = 32;

/// O que um manifesto afirma, conferido ou a assinar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateManifest {
    /// Versão, como `0.2.0`.
    pub version: String,
    /// Nome do arquivo anexado (o zip).
    pub file: String,
    pub sha256: [u8; 32],
}

impl UpdateManifest {
    /// Os bytes assinados: sempre os mesmos para os mesmos campos.
    fn mensagem(&self) -> Vec<u8> {
        format!(
            "{CABECALHO}\nversao={}\narquivo={}\nsha256={}\n",
            self.version,
            self.file,
            hex(&self.sha256)
        )
        .into_bytes()
    }

    /// O texto do manifesto, assinado com `privada` (32 bytes Ed25519).
    pub fn sign(&self, privada: &[u8; 32]) -> String {
        let chave = SigningKey::from_bytes(privada);
        let assinatura = chave.sign(&self.mensagem());
        format!(
            "{CABECALHO}\nversao = {}\narquivo = {}\nsha256 = {}\nassinatura = {}\n",
            self.version,
            self.file,
            hex(&self.sha256),
            hex(&assinatura.to_bytes())
        )
    }

    /// Lê um manifesto e confere a assinatura com `publica`. Só devolve o que
    /// foi assinado por quem tem a chave privada.
    pub fn verify(texto: &str, publica: &[u8; UPDATE_PUBLIC_KEY_LEN]) -> Result<Self, CryptoError> {
        let mut linhas = texto.lines().map(str::trim).filter(|l| !l.is_empty());
        if linhas.next() != Some(CABECALHO) {
            return Err(CryptoError::Manifest);
        }
        let mut campo = |nome: &str| -> Result<String, CryptoError> {
            let l = linhas.next().ok_or(CryptoError::Manifest)?;
            let (k, v) = l.split_once('=').ok_or(CryptoError::Manifest)?;
            match k.trim() == nome {
                true => Ok(v.trim().to_string()),
                false => Err(CryptoError::Manifest),
            }
        };
        let version = campo("versao")?;
        let file = campo("arquivo")?;
        let sha256: [u8; 32] = de_hex(&campo("sha256")?)
            .and_then(|b| b.try_into().ok())
            .ok_or(CryptoError::Manifest)?;
        let assinatura: [u8; 64] = de_hex(&campo("assinatura")?)
            .and_then(|b| b.try_into().ok())
            .ok_or(CryptoError::Manifest)?;
        let m = Self {
            version,
            file,
            sha256,
        };
        let chave = VerifyingKey::from_bytes(publica).map_err(|_| CryptoError::Auth)?;
        chave
            .verify(&m.mensagem(), &Signature::from_bytes(&assinatura))
            .map_err(|_| CryptoError::Auth)?;
        Ok(m)
    }
}

/// A chave pública de uma chave privada (para embutir no programa).
pub fn update_public_key(privada: &[u8; 32]) -> [u8; UPDATE_PUBLIC_KEY_LEN] {
    SigningKey::from_bytes(privada).verifying_key().to_bytes()
}

/// Bytes em hexadecimal minúsculo.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hexadecimal (maiúsculo ou minúsculo) em bytes.
pub fn de_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn privada() -> [u8; 32] {
        let mut k = [0u8; 32];
        crate::random_bytes(&mut k).unwrap();
        k
    }

    fn exemplo() -> UpdateManifest {
        UpdateManifest {
            version: "0.2.0".into(),
            file: "FlameScreenSharing.zip".into(),
            sha256: [7; 32],
        }
    }

    #[test]
    fn assinado_confere_com_a_chave_publica() {
        let k = privada();
        let texto = exemplo().sign(&k);
        assert_eq!(
            UpdateManifest::verify(&texto, &update_public_key(&k)).unwrap(),
            exemplo()
        );
        // Com CRLF e linhas em branco (baixado ou editado no Windows) também.
        let crlf = texto.replace('\n', "\r\n\r\n");
        assert_eq!(
            UpdateManifest::verify(&crlf, &update_public_key(&k)).unwrap(),
            exemplo()
        );
    }

    #[test]
    fn outra_chave_ou_campo_trocado_nao_passa() {
        let k = privada();
        let texto = exemplo().sign(&k);
        let outra = update_public_key(&privada());
        assert_eq!(UpdateManifest::verify(&texto, &outra), Err(CryptoError::Auth));
        let publica = update_public_key(&k);
        // Versão trocada (zip antigo servido como novo), arquivo ou soma trocados.
        for (de, para) in [
            ("versao = 0.2.0", "versao = 9.9.9"),
            ("arquivo = FlameScreenSharing.zip", "arquivo = outro.zip"),
            ("sha256 = 0707", "sha256 = 0807"),
        ] {
            let alterado = texto.replace(de, para);
            assert_ne!(alterado, texto, "{de}");
            assert_eq!(
                UpdateManifest::verify(&alterado, &publica),
                Err(CryptoError::Auth),
                "{de}"
            );
        }
        assert_eq!(
            UpdateManifest::verify("lixo", &publica),
            Err(CryptoError::Manifest)
        );
        let sem_assinatura: String = texto.lines().take(4).collect::<Vec<_>>().join("\n");
        assert_eq!(
            UpdateManifest::verify(&sem_assinatura, &publica),
            Err(CryptoError::Manifest)
        );
    }

    #[test]
    fn hexadecimal() {
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(de_hex("00FF10"), Some(vec![0, 255, 16]));
        assert_eq!(de_hex("0"), None);
        assert_eq!(de_hex("zz"), None);
    }
}
