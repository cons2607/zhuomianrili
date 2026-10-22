//! M4 主题系统：从图片提取主色调，生成色板令牌

use image::{GenericImageView, DynamicImage};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ThemeTokens {
    pub primary: String,
    pub primary_strong: String,
    pub accent: String,
    pub bg: String,
    pub surface: String,
    pub text: String,
    pub text_sub: String,
    pub border: String,
    pub mode: String,
}

#[derive(Clone, Copy, Debug)]
struct Rgb { r: u8, g: u8, b: u8 }

impl Rgb {
    fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
    fn luminance(&self) -> f64 {
        0.2126 * (self.r as f64 / 255.0) + 0.7152 * (self.g as f64 / 255.0) + 0.0722 * (self.b as f64 / 255.0)
    }
}

fn extract_dominant_color(img: &DynamicImage) -> Rgb {
    let small = img.resize_exact(100, 100, image::imageops::FilterType::Triangle);
    let mut pixels: Vec<Rgb> = Vec::new();
    for (_, _, pixel) in small.pixels() {
        let [r, g, b, _] = pixel.0;
        if r < 20 && g < 20 && b < 20 { continue; }
        if r > 235 && g > 235 && b > 235 { continue; }
        pixels.push(Rgb { r, g, b });
    }
    if pixels.is_empty() { return Rgb { r: 74, g: 144, b: 226 }; }
    let k = 5.min(pixels.len());
    let mut centers: Vec<Rgb> = pixels.iter().take(k).cloned().collect();
    for _ in 0..5 {
        let mut clusters: Vec<Vec<Rgb>> = vec![Vec::new(); k];
        for pixel in &pixels {
            let mut min_dist = f64::MAX;
            let mut idx = 0;
            for (i, center) in centers.iter().enumerate() {
                let dr = pixel.r as f64 - center.r as f64;
                let dg = pixel.g as f64 - center.g as f64;
                let db = pixel.b as f64 - center.b as f64;
                let dist = dr*dr + dg*dg + db*db;
                if dist < min_dist { min_dist = dist; idx = i; }
            }
            clusters[idx].push(*pixel);
        }
        for (i, cluster) in clusters.iter().enumerate() {
            if cluster.is_empty() { continue; }
            let avg_r = cluster.iter().map(|p| p.r as u32).sum::<u32>() / cluster.len() as u32;
            let avg_g = cluster.iter().map(|p| p.g as u32).sum::<u32>() / cluster.len() as u32;
            let avg_b = cluster.iter().map(|p| p.b as u32).sum::<u32>() / cluster.len() as u32;
            centers[i] = Rgb { r: avg_r as u8, g: avg_g as u8, b: avg_b as u8 };
        }
    }
    centers.into_iter().max_by(|a, b| {
        let sa = { let max = a.r.max(a.g).max(a.b) as f64; let min = a.r.min(a.g).min(a.b) as f64; if max == 0.0 { 0.0 } else { (max - min) / max } };
        let sb = { let max = b.r.max(b.g).max(b.b) as f64; let min = b.r.min(b.g).min(b.b) as f64; if max == 0.0 { 0.0 } else { (max - min) / max } };
        sa.partial_cmp(&sb).unwrap()
    }).unwrap_or(Rgb { r: 74, g: 144, b: 226 })
}

fn generate_palette(primary: Rgb) -> ThemeTokens {
    let is_dark = primary.luminance() < 0.5;
    let mode = if is_dark { "dark" } else { "light" };
    let (bg, surface, text, text_sub, border) = if is_dark {
        (Rgb { r: (primary.r as f64 * 0.3) as u8, g: (primary.g as f64 * 0.3) as u8, b: (primary.b as f64 * 0.3) as u8 },
         Rgb { r: (primary.r as f64 * 0.5) as u8, g: (primary.g as f64 * 0.5) as u8, b: (primary.b as f64 * 0.5) as u8 },
         Rgb { r: 232, g: 234, b: 237 }, Rgb { r: 154, g: 160, b: 166 }, Rgb { r: 42, g: 46, b: 53 })
    } else {
        (Rgb { r: (primary.r as f64 + (255.0 - primary.r as f64) * 0.85) as u8, g: (primary.g as f64 + (255.0 - primary.g as f64) * 0.85) as u8, b: (primary.b as f64 + (255.0 - primary.b as f64) * 0.85) as u8 },
         Rgb { r: (primary.r as f64 + (255.0 - primary.r as f64) * 0.75) as u8, g: (primary.g as f64 + (255.0 - primary.g as f64) * 0.75) as u8, b: (primary.b as f64 + (255.0 - primary.b as f64) * 0.75) as u8 },
         Rgb { r: 32, g: 33, b: 36 }, Rgb { r: 95, g: 99, b: 104 }, Rgb { r: 218, g: 220, b: 224 })
    };
    ThemeTokens {
        primary: primary.to_hex(),
        primary_strong: Rgb { r: (primary.r as f64 * 1.15).min(255.0) as u8, g: (primary.g as f64 * 1.15).min(255.0) as u8, b: (primary.b as f64 * 1.15).min(255.0) as u8 }.to_hex(),
        accent: Rgb { r: ((primary.r as f64 * 1.3).min(255.0)) as u8, g: primary.g, b: ((primary.b as f64 * 1.2).min(255.0)) as u8 }.to_hex(),
        bg: bg.to_hex(), surface: surface.to_hex(), text: text.to_hex(),
        text_sub: text_sub.to_hex(), border: border.to_hex(), mode: mode.to_string(),
    }
}

pub fn extract_theme_from_image(path: &str) -> Result<ThemeTokens, String> {
    let img = image::open(path).map_err(|e| format!("打开图片失败: {e}"))?;
    let primary = extract_dominant_color(&img);
    Ok(generate_palette(primary))
}

/// 从内存字节提取主题（前端 FileReader 读文件后直传，绕过 WebView2 不暴露 file.path 的限制）
pub fn extract_theme_from_bytes(data: &[u8]) -> Result<ThemeTokens, String> {
    let img = image::load_from_memory(data).map_err(|e| format!("解码图片失败: {e}"))?;
    let primary = extract_dominant_color(&img);
    Ok(generate_palette(primary))
}
