//! Comparação de números de sequência com *wrap-around*.
//!
//! O `frame_id` é um `u32` que cresce 1 por frame. A 60 fps ele só daria a
//! volta depois de ~2,2 anos ligados, mas tratamos o caso mesmo assim para
//! nunca ter um bug "impossível" e para permitir IDs iniciais aleatórios.
//!
//! Usamos a aritmética de números seriais da RFC 1982: `a` é "mais novo" que
//! `b` se a distância de `b` até `a`, andando para frente, for menor que
//! metade do espaço (2³¹).

/// Retorna `true` se `a` é estritamente mais novo que `b`.
///
/// ```
/// use fluxo_proto::seq::is_newer;
/// assert!(is_newer(5, 3));
/// assert!(!is_newer(3, 5));
/// assert!(is_newer(2, u32::MAX)); // deu a volta: 2 vem depois de u32::MAX
/// assert!(!is_newer(7, 7));
/// ```
#[inline]
pub fn is_newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < (1 << 31)
}

/// Retorna `true` se `a` é mais novo ou igual a `b`.
#[inline]
pub fn is_newer_or_equal(a: u32, b: u32) -> bool {
    a == b || is_newer(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordem_simples() {
        assert!(is_newer(1, 0));
        assert!(!is_newer(0, 1));
        assert!(is_newer_or_equal(4, 4));
    }

    #[test]
    fn wrap_around() {
        assert!(is_newer(0, u32::MAX));
        assert!(is_newer(10, u32::MAX - 10));
        assert!(!is_newer(u32::MAX - 10, 10));
    }
}
