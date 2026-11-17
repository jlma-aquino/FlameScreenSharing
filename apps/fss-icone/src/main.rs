//! Desenha o ícone do FlameScreenSharing e grava `pacote/icone/fss.ico`.
//!
//! ```text
//! cargo run -p fss-icone                 grava pacote/icone/fss.ico
//! cargo run -p fss-icone -- SAIDA.ico    grava em outro lugar
//! ```
//!
//! O desenho (monitor, chama na tela e sinal de transmissão no canto) está
//! aqui em coordenadas de 0 a 256 e é rasterizado direto em cada tamanho do
//! .ico, com 8x8 amostras por pixel (bordas suaves também no 16x16). Sem
//! bibliotecas de imagem: o .ico leva bitmaps BGRA de 32 bits.

use std::f32::consts::PI;

const TAMANHOS: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
const AMOSTRAS: u32 = 8;

#[derive(Clone, Copy)]
struct Cor(f32, f32, f32);

fn hex(c: u32) -> Cor {
    Cor(
        ((c >> 16) & 255) as f32 / 255.0,
        ((c >> 8) & 255) as f32 / 255.0,
        (c & 255) as f32 / 255.0,
    )
}

fn mistura(a: Cor, b: Cor, t: f32) -> Cor {
    let t = t.clamp(0.0, 1.0);
    Cor(
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// Uma forma: diz se o ponto (em 0..256) está dentro.
type Forma = Box<dyn Fn(f32, f32) -> bool>;
/// A cor da forma no ponto (gradientes).
type Pintura = Box<dyn Fn(f32, f32) -> Cor>;

fn retangulo_arredondado(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Forma {
    Box::new(move |x, y| {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    })
}

fn circulo(cx: f32, cy: f32, r: f32) -> Forma {
    Box::new(move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r)
}

/// Arco grosso (anel entre `r - g/2` e `r + g/2`) entre os ângulos `a0` e `a1`
/// (graus, 0 = direita, -90 = cima), com as pontas arredondadas.
fn arco(cx: f32, cy: f32, r: f32, g: f32, a0: f32, a1: f32) -> Forma {
    let (a0r, a1r) = (a0 * PI / 180.0, a1 * PI / 180.0);
    let pontas = [
        (cx + r * a0r.cos(), cy + r * a0r.sin()),
        (cx + r * a1r.cos(), cy + r * a1r.sin()),
    ];
    Box::new(move |x, y| {
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let a = (y - cy).atan2(x - cx);
        let dentro_do_anel = (d - r).abs() <= g / 2.0;
        let dentro_do_angulo = a >= a0r && a <= a1r;
        (dentro_do_anel && dentro_do_angulo)
            || pontas
                .iter()
                .any(|(px, py)| (x - px).powi(2) + (y - py).powi(2) <= (g / 2.0).powi(2))
    })
}

/// Caminho fechado de curvas de Bézier cúbicas: `inicio` e, para cada curva,
/// os dois pontos de controle e o fim. Preenchido pela regra par-ímpar.
fn caminho(inicio: (f32, f32), curvas: &[[(f32, f32); 3]]) -> Forma {
    let mut pontos = vec![inicio];
    let mut atual = inicio;
    for [c1, c2, fim] in curvas {
        for i in 1..=24 {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let p = |a: f32, b: f32, c: f32, d: f32| {
                u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
            };
            pontos.push((p(atual.0, c1.0, c2.0, fim.0), p(atual.1, c1.1, c2.1, fim.1)));
        }
        atual = *fim;
    }
    Box::new(move |x, y| {
        let mut dentro = false;
        let n = pontos.len();
        for i in 0..n {
            let (xi, yi) = pontos[i];
            let (xj, yj) = pontos[(i + n - 1) % n];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                dentro = !dentro;
            }
        }
        dentro
    })
}

fn deslocar(pontos: &[[(f32, f32); 3]], dx: f32, dy: f32) -> Vec<[(f32, f32); 3]> {
    pontos.iter().map(|c| c.map(|(x, y)| (x + dx, y + dy))).collect()
}

/// As camadas, de baixo para cima. `recorte`: tira (deixa transparente) o que
/// já foi pintado embaixo, para o sinal não se misturar com o monitor.
struct Camada {
    forma: Forma,
    pintura: Pintura,
    recorte: bool,
}

fn camadas() -> Vec<Camada> {
    let solida = |c: u32| -> Pintura {
        let c = hex(c);
        Box::new(move |_, _| c)
    };
    let mut l = Vec::new();

    // Monitor: moldura, tela, pé e base.
    let (mx0, my0, mx1, my1) = (10.0, 70.0, 198.0, 200.0);
    l.push(Camada {
        forma: retangulo_arredondado(88.0, 196.0, 120.0, 226.0, 3.0),
        pintura: solida(0x3a4150),
        recorte: false,
    });
    l.push(Camada {
        forma: retangulo_arredondado(54.0, 222.0, 154.0, 240.0, 9.0),
        pintura: solida(0x3a4150),
        recorte: false,
    });
    l.push(Camada {
        forma: retangulo_arredondado(mx0, my0, mx1, my1, 18.0),
        pintura: {
            let (a, b) = (hex(0x4a5263), hex(0x2c313c));
            Box::new(move |_, y| mistura(a, b, (y - my0) / (my1 - my0)))
        },
        recorte: false,
    });
    l.push(Camada {
        forma: retangulo_arredondado(mx0 + 12.0, my0 + 12.0, mx1 - 12.0, my1 - 12.0, 8.0),
        pintura: {
            let (a, b) = (hex(0x1b1f29), hex(0x2a1712));
            Box::new(move |_, y| mistura(a, b, (y - my0) / (my1 - my0)))
        },
        recorte: false,
    });

    // Chama no meio da tela: externa (vermelho-laranja) e interna (amarela).
    let (dx, dy) = (-8.0, 4.0);
    let externa = deslocar(
        &[
            [(82.0, 182.0), (68.0, 160.0), (72.0, 138.0)],
            [(76.0, 116.0), (96.0, 104.0), (100.0, 80.0)],
            [(112.0, 92.0), (116.0, 104.0), (114.0, 118.0)],
            [(122.0, 112.0), (126.0, 104.0), (128.0, 96.0)],
            [(146.0, 114.0), (154.0, 134.0), (152.0, 150.0)],
            [(150.0, 170.0), (136.0, 182.0), (112.0, 182.0)],
        ],
        dx,
        dy,
    );
    l.push(Camada {
        forma: caminho((112.0 + dx, 182.0 + dy), &externa),
        pintura: {
            let (a, b) = (hex(0xff3d1f), hex(0xff9a1f));
            Box::new(move |_, y| mistura(a, b, (y - 80.0) / 105.0))
        },
        recorte: false,
    });
    let interna = deslocar(
        &[
            [(96.0, 180.0), (88.0, 168.0), (90.0, 156.0)],
            [(92.0, 144.0), (104.0, 138.0), (106.0, 124.0)],
            [(116.0, 134.0), (120.0, 142.0), (118.0, 152.0)],
            [(124.0, 150.0), (128.0, 146.0), (130.0, 140.0)],
            [(136.0, 150.0), (136.0, 162.0), (132.0, 170.0)],
            [(128.0, 177.0), (120.0, 180.0), (112.0, 180.0)],
        ],
        dx,
        dy,
    );
    l.push(Camada {
        forma: caminho((112.0 + dx, 180.0 + dy), &interna),
        pintura: {
            let (a, b) = (hex(0xffb42a), hex(0xffe27a));
            Box::new(move |_, y| mistura(a, b, (y - 120.0) / 64.0))
        },
        recorte: false,
    });

    // Sinal de Wi-Fi em cima, à direita: um ponto e três arcos em leque,
    // com uma folga transparente em volta.
    let (cx, cy) = (176.0, 64.0);
    let arcos = [(21.0, 11.0), (39.0, 11.0), (57.0, 11.0)];
    let folga = 7.0;
    l.push(Camada {
        forma: circulo(cx, cy, 9.0 + folga),
        pintura: solida(0),
        recorte: true,
    });
    for (r, g) in arcos {
        l.push(Camada {
            forma: arco(cx, cy, r, g + 2.0 * folga, -135.0, -45.0),
            pintura: solida(0),
            recorte: true,
        });
    }
    let sinal = |_: f32, _: f32| hex(0xff8a1f);
    l.push(Camada {
        forma: circulo(cx, cy, 9.0),
        pintura: Box::new(sinal),
        recorte: false,
    });
    for (r, g) in arcos {
        l.push(Camada {
            forma: arco(cx, cy, r, g, -135.0, -45.0),
            pintura: Box::new(sinal),
            recorte: false,
        });
    }
    l
}

/// Rasteriza num quadrado `lado` x `lado`: RGBA, de cima para baixo.
fn desenhar(lado: u32, camadas: &[Camada]) -> Vec<u8> {
    let escala = 256.0 / lado as f32;
    let mut rgba = vec![0u8; (lado * lado * 4) as usize];
    for py in 0..lado {
        for px in 0..lado {
            // Composição "por cima" em cada amostra, depois a média (cor pré-multiplicada).
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..AMOSTRAS {
                for sx in 0..AMOSTRAS {
                    let x = (px as f32 + (sx as f32 + 0.5) / AMOSTRAS as f32) * escala;
                    let y = (py as f32 + (sy as f32 + 0.5) / AMOSTRAS as f32) * escala;
                    let mut cor = Cor(0.0, 0.0, 0.0);
                    let mut opaco = false;
                    for c in camadas {
                        if (c.forma)(x, y) {
                            if c.recorte {
                                opaco = false;
                            } else {
                                cor = (c.pintura)(x, y);
                                opaco = true;
                            }
                        }
                    }
                    if opaco {
                        r += cor.0;
                        g += cor.1;
                        b += cor.2;
                        a += 1.0;
                    }
                }
            }
            let n = (AMOSTRAS * AMOSTRAS) as f32;
            let i = ((py * lado + px) * 4) as usize;
            if a > 0.0 {
                rgba[i] = (r / a * 255.0).round() as u8;
                rgba[i + 1] = (g / a * 255.0).round() as u8;
                rgba[i + 2] = (b / a * 255.0).round() as u8;
            }
            rgba[i + 3] = (a / n * 255.0).round() as u8;
        }
    }
    rgba
}

/// Um bitmap do .ico: cabeçalho BMP (altura dobrada), BGRA de baixo para cima
/// e a máscara AND (zerada: a transparência vem do alfa).
fn bitmap(lado: u32, rgba: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    let le32 = |v: &mut Vec<u8>, n: u32| v.extend_from_slice(&n.to_le_bytes());
    le32(&mut v, 40);
    le32(&mut v, lado);
    le32(&mut v, lado * 2);
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    for _ in 0..6 {
        le32(&mut v, 0);
    }
    for y in (0..lado).rev() {
        for x in 0..lado {
            let i = ((y * lado + x) * 4) as usize;
            v.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    let linha_mascara = lado.div_ceil(32) * 4;
    v.extend(std::iter::repeat_n(0u8, (linha_mascara * lado) as usize));
    v
}

fn ico(imagens: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&[0, 0, 1, 0]);
    v.extend_from_slice(&(imagens.len() as u16).to_le_bytes());
    let mut deslocamento = 6 + 16 * imagens.len() as u32;
    let dados: Vec<Vec<u8>> = imagens.iter().map(|(l, rgba)| bitmap(*l, rgba)).collect();
    for ((lado, _), d) in imagens.iter().zip(&dados) {
        let l = if *lado >= 256 { 0 } else { *lado as u8 };
        v.extend_from_slice(&[l, l, 0, 0]);
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&(d.len() as u32).to_le_bytes());
        v.extend_from_slice(&deslocamento.to_le_bytes());
        deslocamento += d.len() as u32;
    }
    for d in dados {
        v.extend(d);
    }
    v
}

fn main() {
    let saida = std::env::args()
        .nth(1)
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../pacote/icone/fss.ico").to_string());
    let c = camadas();
    let imagens: Vec<(u32, Vec<u8>)> = TAMANHOS.iter().map(|&l| (l, desenhar(l, &c))).collect();
    if let Some(p) = std::path::Path::new(&saida).parent() {
        std::fs::create_dir_all(p).expect("criar a pasta do ícone");
    }
    std::fs::write(&saida, ico(&imagens)).expect("gravar o ícone");
    println!("Ícone gravado: {saida} ({} tamanhos)", imagens.len());
}
