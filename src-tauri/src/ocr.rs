//! M6 OCR：调用 Windows 系统 OCR（WinRT Windows.Media.Ocr）提取图片中的文字
//!
//! 使用 windows crate 0.62 + windows-future 0.3。
//! 因为 IAsyncOperation 没有 .get()，用轮询 Status() 的方式阻塞等待。

use windows::{
    core::HSTRING,
    Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, BitmapAlphaMode},
    Media::Ocr::OcrEngine,
    Storage::{StorageFile, FileAccessMode},
};
use windows_future::{AsyncStatus, IAsyncOperation};

/// 轮询等待 WinRT 异步操作完成，然后取回结果
fn wait_async<T: windows::core::RuntimeType + 'static>(
    op: &IAsyncOperation<T>,
) -> Result<T, String> {
    loop {
        let status = op.Status().map_err(|e| format!("查询状态失败: {e}"))?;
        match status {
            AsyncStatus(1) => return op.GetResults().map_err(|e| format!("获取结果失败: {e}")),
            AsyncStatus(2) => return Err("操作被取消".into()),
            AsyncStatus(3) => {
                let code = op.ErrorCode().map(|c| c.0).unwrap_or(0);
                return Err(format!("操作失败，HRESULT=0x{code:08X}"));
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

/// 从图片文件路径 OCR 提取文字
pub fn ocr_file(image_path: &str) -> Result<String, String> {
    if !std::path::Path::new(image_path).exists() {
        return Err(format!("文件不存在: {image_path}"));
    }

    let hpath = HSTRING::from(image_path);

    // 打开文件
    let file = wait_async(
        &StorageFile::GetFileFromPathAsync(&hpath)
            .map_err(|e| format!("打开文件失败: {e}"))?,
    )?;

    // 打开随机访问流
    let stream = wait_async(
        &file.OpenAsync(FileAccessMode::Read)
            .map_err(|e| format!("打开流失败: {e}"))?,
    )?;

    // 创建位图解码器
    let decoder = wait_async(
        &BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| format!("创建解码器失败: {e}"))?,
    )?;

    // 获取 SoftwareBitmap（尝试无参版本）
    let bitmap = wait_async(
        &decoder.GetSoftwareBitmapAsync()
            .map_err(|e| format!("获取位图失败: {e}"))?,
    )?;

    // 创建 OCR 引擎：优先中文简体，回退到用户配置语言
    let lang = windows::Globalization::Language::CreateLanguage(&HSTRING::from("zh-Hans-CN"))
        .unwrap_or_else(|_| windows::Globalization::Language::CreateLanguage(&HSTRING::from("en-US")).unwrap());
    let engine = OcrEngine::TryCreateFromLanguage(&lang)
        .or_else(|_| OcrEngine::TryCreateFromUserProfileLanguages())
        .map_err(|e| format!("创建 OCR 引擎失败: {e}"))?;

    // 执行识别
    let result = wait_async(
        &engine.RecognizeAsync(&bitmap)
            .map_err(|e| format!("识别调用失败: {e}"))?,
    )?;

    let text = result.Text().map_err(|e| format!("获取文本失败: {e}"))?;
    let trimmed = text.to_string_lossy().trim().to_string();
    if trimmed.is_empty() {
        Err("OCR 未识别到文字".into())
    } else {
        Ok(trimmed)
    }
}

/// 从剪贴板读取图片并 OCR
pub fn ocr_clipboard() -> Result<String, String> {
    // 先用 PowerShell 把剪贴板图片存为临时 PNG
    let temp_dir = std::env::temp_dir().join("desktop_calendar_ocr");
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let temp_path = temp_dir.join(format!("clip_{}.png", uuid::Uuid::new_v4().simple()));
    let temp_str = temp_path.to_string_lossy().replace('\\', "\\\\");

    let ps = format!(
        r#"Add-Type -AssemblyName System.Windows.Forms; $img = [System.Windows.Forms.Clipboard]::GetImage(); if ($null -eq $img) {{ exit 1 }}; $img.Save('{temp_str}', [System.Drawing.Imaging.ImageFormat]::Png)"#
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &ps])
        .output()
        .map_err(|e| format!("启动 PowerShell 失败: {e}"))?;
    if !output.status.success() || !temp_path.exists() {
        return Err("剪贴板中没有图片".into());
    }

    let result = ocr_file(temp_path.to_str().unwrap_or_default());
    let _ = std::fs::remove_file(&temp_path);
    result
}
