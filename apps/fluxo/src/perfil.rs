//! Perfil (ADR-023): o nome e a foto que os outros do grupo veem.
//!
//! O perfil é ligado ao número de membro deste FSS (sorteado na instalação e
//! guardado nas configurações), não ao IP: o IP muda, é o mesmo para quem
//! mora junto e é o do servidor para quem entra pelo repasse.
//!
//! A foto escolhida é cortada no meio em quadrado, reduzida para
//! [`LADO`]×[`LADO`] e gravada como JPEG pequeno (até
//! [`fluxo_net::MAX_PHOTO_JPEG`] bytes) em `foto.jpg`, na pasta das
//! configurações. Quem não tem foto aparece com as iniciais numa cor tirada do
//! número de membro. Tudo pelo WIC do próprio Windows: nenhuma biblioteca de
//! imagem a mais.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32};

/// Lado da foto, em pixels.
pub const LADO: u32 = 80;

/// Onde fica a foto deste FSS.
pub fn caminho_da_foto() -> Option<PathBuf> {
    Some(crate::config::caminho()?.parent()?.join("foto.jpg"))
}

/// Onde ficam as fotos dos outros membros (cifradas, ver `GroupClient`).
pub fn pasta_das_fotos() -> Option<PathBuf> {
    Some(crate::config::caminho()?.parent()?.join("fotos"))
}

/// A foto deste FSS (o JPEG), se houver.
pub fn ler_foto() -> Option<Vec<u8>> {
    let j = std::fs::read(caminho_da_foto()?).ok()?;
    (!j.is_empty() && j.len() <= fluxo_net::MAX_PHOTO_JPEG).then_some(j)
}

/// Prepara a imagem `origem` e grava como a foto deste FSS. Devolve o JPEG.
pub fn trocar_foto(origem: &Path) -> Result<Vec<u8>, String> {
    let jpeg = preparar(origem)?;
    let destino = caminho_da_foto().ok_or("sem pasta para guardar a foto")?;
    if let Some(p) = destino.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&destino, &jpeg).map_err(|e| format!("não consegui guardar a foto: {e}"))?;
    Ok(jpeg)
}

/// Tira a foto deste FSS.
pub fn tirar_foto() {
    if let Some(c) = caminho_da_foto() {
        let _ = std::fs::remove_file(c);
    }
}

/// As iniciais de um nome: "Ana Lúcia" → "AL", "joao" → "J".
pub fn iniciais(nome: &str) -> String {
    let partes: Vec<&str> = nome.split_whitespace().collect();
    let letra = |p: &str| p.chars().next().map(|c| c.to_uppercase().collect::<String>());
    match partes.as_slice() {
        [] => "?".into(),
        [um] => letra(um).unwrap_or_default(),
        [primeiro, .., ultimo] => format!(
            "{}{}",
            letra(primeiro).unwrap_or_default(),
            letra(ultimo).unwrap_or_default()
        ),
    }
}

/// Cor do círculo de quem não tem foto: fixa para cada membro.
pub fn cor_do_membro(membro: u64) -> Color32 {
    const CORES: [Color32; 8] = [
        Color32::from_rgb(0xE0, 0x5A, 0x47),
        Color32::from_rgb(0xE8, 0x8F, 0x2E),
        Color32::from_rgb(0xC9, 0xA2, 0x27),
        Color32::from_rgb(0x4C, 0xAF, 0x6E),
        Color32::from_rgb(0x2E, 0xA3, 0xA8),
        Color32::from_rgb(0x4A, 0x7F, 0xD6),
        Color32::from_rgb(0x8E, 0x63, 0xD2),
        Color32::from_rgb(0xD0, 0x5C, 0x9E),
    ];
    let mistura = membro ^ (membro >> 29) ^ (membro >> 47);
    CORES[(mistura % CORES.len() as u64) as usize]
}

/// Um círculo com a foto (se houver) ou com as iniciais (clicável).
pub fn avatar(
    ui: &mut egui::Ui,
    membro: u64,
    nome: &str,
    foto: Option<&egui::TextureHandle>,
    tamanho: f32,
) -> egui::Response {
    let (rect, resposta) = ui.allocate_exact_size(egui::vec2(tamanho, tamanho), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        pintar_avatar(ui, rect, membro, nome, foto, false);
    }
    resposta
}

/// Pinta o círculo da foto (ou das iniciais) em `rect`. `apagado`: offline.
pub fn pintar_avatar(
    ui: &egui::Ui,
    rect: egui::Rect,
    membro: u64,
    nome: &str,
    foto: Option<&egui::TextureHandle>,
    apagado: bool,
) {
    let tamanho = rect.width().min(rect.height());
    match foto {
        Some(t) => {
            let tinta = if apagado {
                Color32::from_white_alpha(110)
            } else {
                Color32::WHITE
            };
            egui::Image::new(t)
                .corner_radius(egui::CornerRadius::same((tamanho / 2.0).round() as u8))
                .tint(tinta)
                .paint_at(ui, rect);
        }
        None => {
            let cor = cor_do_membro(membro);
            let pintor = ui.painter();
            pintor.circle_filled(
                rect.center(),
                tamanho / 2.0,
                if apagado { cor.gamma_multiply(0.4) } else { cor },
            );
            pintor.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                iniciais(nome),
                egui::FontId::proportional(tamanho * 0.42),
                if apagado {
                    Color32::from_white_alpha(150)
                } else {
                    Color32::WHITE
                },
            );
        }
    }
}

/// Textura de uma foto (JPEG), guardada pelo número da foto: só decodifica
/// quando ela muda.
#[derive(Default)]
pub struct Fotos {
    texturas: std::collections::HashMap<u64, (u32, Option<egui::TextureHandle>)>,
}

impl Fotos {
    /// A textura da foto `id` do membro `membro` (cria na primeira vez).
    pub fn textura(
        &mut self,
        ctx: &egui::Context,
        membro: u64,
        id: u32,
        jpeg: &[u8],
    ) -> Option<egui::TextureHandle> {
        if let Some((guardada, t)) = self.texturas.get(&membro) {
            if *guardada == id {
                return t.clone();
            }
        }
        let t = decodificar(jpeg).map(|img| {
            ctx.load_texture(
                format!("foto-{membro:016x}-{id:08x}"),
                img,
                egui::TextureOptions::LINEAR,
            )
        });
        self.texturas.insert(membro, (id, t.clone()));
        t
    }

    /// Esquece a textura de quem não tem mais foto.
    pub fn esquecer(&mut self, membro: u64) {
        self.texturas.remove(&membro);
    }
}

#[cfg(windows)]
pub use wic::{decodificar, escolher_arquivo, preparar};

#[cfg(not(windows))]
pub fn preparar(_: &Path) -> Result<Vec<u8>, String> {
    Err("escolher a foto ainda só funciona no Windows".into())
}

#[cfg(not(windows))]
pub fn decodificar(_: &[u8]) -> Option<egui::ColorImage> {
    None
}

#[cfg(not(windows))]
pub fn escolher_arquivo(_: Option<isize>) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
mod wic {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use eframe::egui;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{GENERIC_READ, HWND};
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::StructuredStorage::PROPBAG2;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, STREAM_SEEK_CUR,
    };
    use windows::Win32::System::Variant::{VARIANT, VT_R4};
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    use super::LADO;

    /// Fundo das partes transparentes (PNG): o cinza escuro do tema.
    const FUNDO: [u8; 3] = [0x2A, 0x26, 0x24];

    fn erro(e: windows::core::Error) -> String {
        format!("não consegui ler a imagem: {}", e.message())
    }

    fn fabrica() -> Result<IWICImagingFactory, String> {
        // SAFETY: inicializar o COM nesta thread (se já estava, com outro
        // modo, segue valendo o de antes) e criar a fábrica do WIC.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(erro)
        }
    }

    /// Abre a imagem (JPEG, PNG, BMP, GIF, TIFF e o que mais o Windows ler),
    /// corta o meio em quadrado, reduz e grava em JPEG pequeno.
    pub fn preparar(origem: &Path) -> Result<Vec<u8>, String> {
        let f = fabrica()?;
        let nome: Vec<u16> = origem.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: chamadas do WIC com objetos válidos e buffers do tamanho certo.
        unsafe {
            let dec = f
                .CreateDecoderFromFilename(
                    PCWSTR(nome.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(|_| "o Windows não reconheceu este arquivo como imagem".to_string())?;
            let quadro = dec.GetFrame(0).map_err(erro)?;
            let (mut l, mut a) = (0u32, 0u32);
            quadro.GetSize(&mut l, &mut a).map_err(erro)?;
            if l == 0 || a == 0 {
                return Err("imagem vazia".into());
            }
            let lado = l.min(a);
            let corte = f.CreateBitmapClipper().map_err(erro)?;
            corte
                .Initialize(
                    &quadro,
                    &WICRect {
                        X: ((l - lado) / 2) as i32,
                        Y: ((a - lado) / 2) as i32,
                        Width: lado as i32,
                        Height: lado as i32,
                    },
                )
                .map_err(erro)?;
            let escala = f.CreateBitmapScaler().map_err(erro)?;
            escala
                .Initialize(&corte, LADO, LADO, WICBitmapInterpolationModeFant)
                .map_err(erro)?;
            let px = pixels(&f, &escala, &GUID_WICPixelFormat32bppPBGRA, LADO, LADO)?;
            // Pré-multiplicado: soma o fundo onde é transparente.
            let mut bgr = Vec::with_capacity((LADO * LADO * 3) as usize);
            for p in px.chunks_exact(4) {
                let falta = 255 - p[3] as u32;
                for c in 0..3 {
                    bgr.push((p[c] as u32 + FUNDO[2 - c] as u32 * falta / 255).min(255) as u8);
                }
            }
            for qualidade in [0.9f32, 0.8, 0.7, 0.6, 0.5, 0.4] {
                let jpeg = codificar(&f, &bgr, qualidade)?;
                if jpeg.len() <= fluxo_net::MAX_PHOTO_JPEG {
                    return Ok(jpeg);
                }
            }
            Err("não consegui deixar a foto pequena o bastante".into())
        }
    }

    /// Converte `fonte` para `formato` e copia os pixels.
    ///
    /// # Safety
    /// `fonte` precisa ter `l`×`a` pixels.
    unsafe fn pixels(
        f: &IWICImagingFactory,
        fonte: &IWICBitmapSource,
        formato: &windows::core::GUID,
        l: u32,
        a: u32,
    ) -> Result<Vec<u8>, String> {
        unsafe {
            let conv = f.CreateFormatConverter().map_err(erro)?;
            conv.Initialize(
                fonte,
                formato,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(erro)?;
            let mut px = vec![0u8; (l * a * 4) as usize];
            conv.CopyPixels(std::ptr::null(), l * 4, &mut px).map_err(erro)?;
            Ok(px)
        }
    }

    /// JPEG de [`LADO`]×[`LADO`] pixels BGR com esta qualidade (0 a 1).
    ///
    /// # Safety
    /// `bgr` precisa ter `LADO * LADO * 3` bytes.
    unsafe fn codificar(f: &IWICImagingFactory, bgr: &[u8], qualidade: f32) -> Result<Vec<u8>, String> {
        let mut saida = vec![0u8; 64 * 1024];
        unsafe {
            let fluxo = f.CreateStream().map_err(erro)?;
            // O WIC escreve neste buffer, que vive até o fim da função.
            fluxo
                .InitializeFromMemory(std::slice::from_raw_parts(saida.as_mut_ptr(), saida.len()))
                .map_err(erro)?;
            let enc = f
                .CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())
                .map_err(erro)?;
            enc.Initialize(&fluxo, WICBitmapEncoderNoCache).map_err(erro)?;
            let mut quadro = None;
            let mut opcoes = None;
            enc.CreateNewFrame(&mut quadro, &mut opcoes).map_err(erro)?;
            let quadro = quadro.ok_or("o WIC não criou o quadro")?;
            if let Some(o) = &opcoes {
                let mut nome: Vec<u16> = "ImageQuality".encode_utf16().chain([0]).collect();
                let opcao = PROPBAG2 {
                    pstrName: PWSTR(nome.as_mut_ptr()),
                    ..Default::default()
                };
                let mut v = VARIANT::default();
                (*v.Anonymous.Anonymous).vt = VT_R4;
                (*v.Anonymous.Anonymous).Anonymous.fltVal = qualidade;
                let _ = o.Write(1, &opcao, &v);
            }
            quadro.Initialize(opcoes.as_ref()).map_err(erro)?;
            quadro.SetSize(LADO, LADO).map_err(erro)?;
            let mut formato = GUID_WICPixelFormat24bppBGR;
            quadro.SetPixelFormat(&mut formato).map_err(erro)?;
            if formato != GUID_WICPixelFormat24bppBGR {
                return Err("o WIC não aceitou o formato da foto".into());
            }
            quadro.WritePixels(LADO, LADO * 3, bgr).map_err(erro)?;
            quadro.Commit().map_err(erro)?;
            enc.Commit().map_err(erro)?;
            let mut fim = 0u64;
            fluxo.Seek(0, STREAM_SEEK_CUR, Some(&mut fim)).map_err(erro)?;
            saida.truncate(fim as usize);
        }
        Ok(saida)
    }

    /// Abre uma foto que chegou do grupo (JPEG pequeno) para mostrar.
    pub fn decodificar(jpeg: &[u8]) -> Option<egui::ColorImage> {
        let f = fabrica().ok()?;
        // SAFETY: o WIC lê de `jpeg`, que vive até o fim da função.
        unsafe {
            let fluxo = f.CreateStream().ok()?;
            fluxo.InitializeFromMemory(jpeg).ok()?;
            let dec = f
                .CreateDecoderFromStream(&fluxo, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
                .ok()?;
            let quadro = dec.GetFrame(0).ok()?;
            let (mut l, mut a) = (0u32, 0u32);
            quadro.GetSize(&mut l, &mut a).ok()?;
            // Foto de perfil é pequena; o resto é lixo.
            if l == 0 || a == 0 || l > 256 || a > 256 {
                return None;
            }
            let px = pixels(&f, &quadro, &GUID_WICPixelFormat32bppRGBA, l, a).ok()?;
            Some(egui::ColorImage::from_rgba_unmultiplied(
                [l as usize, a as usize],
                &px,
            ))
        }
    }

    /// A janela "Abrir" do Windows, só com imagens. `dono`: a janela do FSS.
    pub fn escolher_arquivo(dono: Option<isize>) -> Option<PathBuf> {
        let filtro: Vec<u16> =
            "Imagens\0*.jpg;*.jpeg;*.png;*.bmp;*.gif;*.tif;*.tiff;*.webp;*.heic\0Todos os arquivos\0*.*\0\0"
                .encode_utf16()
                .collect();
        let titulo: Vec<u16> = "Escolher a foto do perfil\0".encode_utf16().collect();
        let mut arquivo = vec![0u16; 1024];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: HWND(dono.unwrap_or(0) as *mut core::ffi::c_void),
            lpstrFilter: PCWSTR(filtro.as_ptr()),
            lpstrFile: PWSTR(arquivo.as_mut_ptr()),
            nMaxFile: arquivo.len() as u32,
            lpstrTitle: PCWSTR(titulo.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        // SAFETY: todos os buffers vivem até o fim da chamada.
        let ok = unsafe { GetOpenFileNameW(&mut ofn) }.as_bool();
        if !ok {
            return None;
        }
        let fim = arquivo.iter().position(|c| *c == 0).unwrap_or(arquivo.len());
        Some(PathBuf::from(String::from_utf16_lossy(&arquivo[..fim])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iniciais_do_nome() {
        assert_eq!(iniciais("Ana Lúcia"), "AL");
        assert_eq!(iniciais("joao"), "J");
        assert_eq!(iniciais("  maria da silva "), "MS");
        assert_eq!(iniciais(""), "?");
        assert_eq!(iniciais("élio"), "É");
    }

    #[test]
    fn cor_fixa_por_membro() {
        assert_eq!(cor_do_membro(42), cor_do_membro(42));
        let cores: std::collections::HashSet<_> = (0..64u64).map(|m| cor_do_membro(m * 7919)).collect();
        assert!(cores.len() > 4, "as cores variam entre membros");
    }

    #[cfg(windows)]
    #[test]
    fn foto_de_qualquer_tamanho_vira_jpeg_pequeno_e_volta() {
        // Uma imagem 300x200 em BMP (o WIC lê), com degradê.
        let (l, a) = (300u32, 200u32);
        let mut bmp = Vec::new();
        let linha = (l * 3).div_ceil(4) * 4;
        let tamanho = 54 + linha * a;
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&tamanho.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&(l as i32).to_le_bytes());
        bmp.extend_from_slice(&(a as i32).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        for y in 0..a {
            for x in 0..l {
                bmp.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
            }
            bmp.resize(bmp.len() + (linha - l * 3) as usize, 0);
        }
        let origem = std::env::temp_dir().join(format!("fss-foto-teste-{}.bmp", std::process::id()));
        std::fs::write(&origem, &bmp).unwrap();
        let jpeg = preparar(&origem).unwrap();
        let _ = std::fs::remove_file(&origem);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "é JPEG");
        assert!(jpeg.len() <= fluxo_net::MAX_PHOTO_JPEG, "{} bytes", jpeg.len());
        let img = decodificar(&jpeg).unwrap();
        assert_eq!(img.size, [LADO as usize, LADO as usize]);
        assert!(decodificar(b"lixo").is_none());
        assert!(preparar(Path::new("C:\\nao\\existe.png")).is_err());
    }
}
