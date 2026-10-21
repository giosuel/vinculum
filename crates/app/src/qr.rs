//! QR codes for sharing a Wi-Fi network: the `WIFI:` text phone cameras understand, drawn as an
//! image for the window.

use qrcodegen::{QrCode, QrCodeEcc};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// The text a phone camera reads to join a network. `;`, `,`, `:`, `\` and `"` are escaped.
pub fn wifi_text(ssid: &str, password: Option<&str>, wep: bool, hidden: bool) -> String {
    let escape = |s: &str| {
        s.chars().flat_map(|c| if ";,:\\\"".contains(c) { vec!['\\', c] } else { vec![c] }).collect::<String>()
    };
    let mut text = format!("WIFI:S:{};", escape(ssid));
    match password {
        Some(p) => text += &format!("T:{};P:{};", if wep { "WEP" } else { "WPA" }, escape(p)),
        None => text += "T:nopass;",
    }
    if hidden {
        text += "H:true;";
    }
    text + ";"
}

/// The code as a black-on-white image, `scale` pixels per module, with the quiet zone around it.
pub fn image(text: &str, scale: u32) -> Option<Image> {
    let code = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let border = 2;
    let modules = code.size() as u32 + 2 * border;
    let side = modules * scale;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(side, side);
    let pixels = buffer.make_mut_slice();
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = ((x / scale) as i32 - border as i32, (y / scale) as i32 - border as i32);
            let dark = code.get_module(mx, my);
            let v = if dark { 0x11 } else { 0xff };
            pixels[(y * side + x) as usize] = Rgba8Pixel { r: v, g: v, b: v, a: 0xff };
        }
    }
    Some(Image::from_rgba8(buffer))
}

#[cfg(test)]
mod tests {
    #[test]
    fn wifi_texts() {
        assert_eq!(super::wifi_text("Lumen Home", Some("pa;ss"), false, false), r"WIFI:S:Lumen Home;T:WPA;P:pa\;ss;;");
        assert_eq!(super::wifi_text("Free", None, false, true), "WIFI:S:Free;T:nopass;H:true;;");
        assert!(super::image("WIFI:S:x;;", 4).is_some());
    }
}
