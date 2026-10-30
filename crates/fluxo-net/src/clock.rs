//! Relógio de 32 bits usado em `timestamp_us`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Agora, em microssegundos do relógio de parede (UTC), truncado para 32
/// bits — dá a volta a cada ~71 minutos. Compare sempre com
/// [`clock_diff_us`], nunca com `<`/`-` direto.
pub fn clock_us() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u32)
        .unwrap_or(0)
}

/// `later − earlier` em µs, correto mesmo quando o relógio deu a volta entre
/// os dois. Negativo se `later` for, na verdade, anterior.
///
/// ```
/// use fluxo_net::clock_diff_us;
/// assert_eq!(clock_diff_us(1_500, 1_000), 500);
/// assert_eq!(clock_diff_us(100, u32::MAX - 99), 200); // atravessou a volta
/// assert_eq!(clock_diff_us(1_000, 1_500), -500);
/// ```
pub fn clock_diff_us(later: u32, earlier: u32) -> i32 {
    later.wrapping_sub(earlier) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relogio_avanca() {
        let a = clock_us();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let d = clock_diff_us(clock_us(), a);
        assert!((4_000..1_000_000).contains(&d), "diferença {d} µs");
    }
}
