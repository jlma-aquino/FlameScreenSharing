//! Escrita de imagens BMP de 32 bits, sem dependências.
//!
//! BMP foi escolhido porque o formato nativo dele (BGRA) é idêntico ao que a
//! captura devolve: dá para gravar os bytes direto, sem conversão.

use std::io::{self, Write};

/// Grava pixels BGRA (linhas contíguas, de cima para baixo) como BMP.
pub fn write_bmp32<W: Write>(mut w: W, width: u32, height: u32, bgra: &[u8]) -> io::Result<()> {
    let image_size = width as usize * height as usize * 4;
    if bgra.len() < image_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "buffer menor que a imagem",
        ));
    }
    const FILE_HEADER: u32 = 14;
    const INFO_HEADER: u32 = 40;
    let offset = FILE_HEADER + INFO_HEADER;
    let file_size = offset as usize + image_size;

    // BITMAPFILEHEADER
    w.write_all(b"BM")?;
    w.write_all(&(file_size as u32).to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?; // reservado
    w.write_all(&offset.to_le_bytes())?;

    // BITMAPINFOHEADER
    w.write_all(&INFO_HEADER.to_le_bytes())?;
    w.write_all(&(width as i32).to_le_bytes())?;
    // Altura negativa = linhas de cima para baixo (a ordem da captura).
    w.write_all(&(-(height as i32)).to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // planos
    w.write_all(&32u16.to_le_bytes())?; // bits por pixel
    w.write_all(&0u32.to_le_bytes())?; // BI_RGB (sem compressão)
    w.write_all(&(image_size as u32).to_le_bytes())?;
    w.write_all(&2835i32.to_le_bytes())?; // 72 DPI horizontal
    w.write_all(&2835i32.to_le_bytes())?; // 72 DPI vertical
    w.write_all(&0u32.to_le_bytes())?; // cores na paleta
    w.write_all(&0u32.to_le_bytes())?; // cores importantes

    w.write_all(&bgra[..image_size])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabecalho_e_tamanho() {
        let mut out = Vec::new();
        write_bmp32(&mut out, 2, 3, &[0xAB; 24]).unwrap();
        assert_eq!(&out[0..2], b"BM");
        assert_eq!(out.len(), 54 + 24);
        assert_eq!(u32::from_le_bytes(out[2..6].try_into().unwrap()), 78);
        assert_eq!(i32::from_le_bytes(out[22..26].try_into().unwrap()), -3);
    }

    #[test]
    fn buffer_curto_da_erro() {
        assert!(write_bmp32(Vec::new(), 10, 10, &[0; 4]).is_err());
    }
}
