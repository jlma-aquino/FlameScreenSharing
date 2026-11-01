//! Conversão de taxa e de canais: [`Converter`].
//!
//! O Opus trabalha em 48 kHz estéreo; a placa de som pode estar em 44,1 kHz,
//! 96 kHz, mono ou 5.1. Interpolação linear entre amostras (simples e
//! barata; para fala, música de jogo e vídeo não se nota) e mistura de
//! canais: mono vira os dois lados; mais de dois, frente esquerda/direita
//! mais o centro (−3 dB) em cada lado.

/// Converte um fluxo contínuo de (`de_taxa`, `de_canais`) para
/// (`para_taxa`, `para_canais`), guardando o estado entre blocos.
#[derive(Debug, Clone)]
pub struct Converter {
    de_taxa: u32,
    de_canais: usize,
    para_taxa: u32,
    para_canais: usize,
    /// Posição da próxima amostra de saída, em amostras de entrada, a partir
    /// da última do bloco anterior (`anterior`).
    pos: f64,
    /// Última amostra (já com os canais de saída) do bloco anterior.
    anterior: Vec<f32>,
}

impl Converter {
    pub fn new(de_taxa: u32, de_canais: usize, para_taxa: u32, para_canais: usize) -> Self {
        Self {
            de_taxa,
            de_canais: de_canais.max(1),
            para_taxa,
            para_canais: para_canais.max(1),
            pos: 1.0,
            anterior: vec![0.0; para_canais.max(1)],
        }
    }

    /// `true` se não há nada a fazer (mesma taxa e canais).
    pub fn is_identity(&self) -> bool {
        self.de_taxa == self.para_taxa && self.de_canais == self.para_canais
    }

    /// Converte um bloco intercalado; devolve as amostras de saída.
    pub fn process(&mut self, entrada: &[f32]) -> Vec<f32> {
        if self.is_identity() {
            return entrada.to_vec();
        }
        // Primeiro os canais, depois a taxa (com menos canais, menos conta).
        let quadros: Vec<f32> = entrada
            .chunks_exact(self.de_canais)
            .flat_map(|q| misturar(q, self.para_canais))
            .collect();
        if self.de_taxa == self.para_taxa {
            return quadros;
        }
        let n = quadros.len() / self.para_canais;
        let passo = self.de_taxa as f64 / self.para_taxa as f64;
        let c = self.para_canais;
        let mut saida = Vec::with_capacity((n as f64 / passo) as usize * c + c);
        // Amostra i da "linha do tempo": 0 = `anterior`, 1.. = o bloco.
        let amostra = |i: usize, canal: usize| -> f32 {
            match i {
                0 => self.anterior[canal],
                i => quadros[(i - 1) * c + canal],
            }
        };
        while self.pos <= n as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            for canal in 0..c {
                let a = amostra(i, canal);
                let b = if i < n { amostra(i + 1, canal) } else { a };
                saida.push(a + (b - a) * frac);
            }
            self.pos += passo;
        }
        self.pos -= n as f64;
        if n > 0 {
            self.anterior.copy_from_slice(&quadros[(n - 1) * c..n * c]);
        }
        saida
    }
}

/// Um quadro de entrada (todos os canais) → `para` canais.
fn misturar(q: &[f32], para: usize) -> Vec<f32> {
    let (e, d) = match q.len() {
        0 => (0.0, 0.0),
        1 => (q[0], q[0]),
        2 => (q[0], q[1]),
        // 5.1, 7.1...: frente E/D + centro a −3 dB (ordem do WAVEFORMATEXTENSIBLE).
        _ => (q[0] + q[2] * 0.707, q[1] + q[2] * 0.707),
    };
    match para {
        1 => vec![(e + d) * 0.5],
        2 => vec![e, d],
        n => {
            let mut v = vec![0.0; n];
            v[0] = e;
            v[1] = d;
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesma_taxa_e_canais_nao_mexe() {
        let mut c = Converter::new(48_000, 2, 48_000, 2);
        assert!(c.is_identity());
        assert_eq!(c.process(&[0.1, 0.2, 0.3, 0.4]), vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_vira_estereo_e_5_1_vira_estereo() {
        let mut c = Converter::new(48_000, 1, 48_000, 2);
        assert_eq!(c.process(&[0.5, -0.5]), vec![0.5, 0.5, -0.5, -0.5]);
        let mut c = Converter::new(48_000, 6, 48_000, 2);
        let s = c.process(&[0.1, 0.2, 1.0, 0.0, 0.0, 0.0]);
        assert!(
            (s[0] - 0.807).abs() < 1e-3 && (s[1] - 0.907).abs() < 1e-3,
            "{s:?}"
        );
    }

    #[test]
    fn taxa_convertida_em_blocos_mantem_a_contagem_e_a_forma() {
        // 1 s de rampa a 44,1 kHz, em blocos de 441 → ~48 000 amostras.
        let mut c = Converter::new(44_100, 1, 48_000, 1);
        let mut saida = Vec::new();
        let entrada: Vec<f32> = (0..44_100).map(|i| i as f32 / 44_100.0).collect();
        for b in entrada.chunks(441) {
            saida.extend(c.process(b));
        }
        assert!((saida.len() as i64 - 48_000).abs() <= 2, "{}", saida.len());
        // Rampa continua rampa: monotônica e sem saltos nas emendas.
        let maior_salto = saida.windows(2).map(|w| w[1] - w[0]).fold(0f32, f32::max);
        assert!(saida.windows(2).all(|w| w[1] >= w[0]));
        assert!(maior_salto < 2.0 / 44_100.0, "{maior_salto}");
    }
}
