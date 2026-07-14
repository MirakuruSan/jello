use image::{ImageBuffer, Rgba};
use tauri::Manager;
use windows::Win32::Graphics::Gdi::{
    GetDC, CreateCompatibleDC, CreateCompatibleBitmap, SelectObject, BitBlt, DeleteObject, DeleteDC, ReleaseDC, SRCCOPY, BITMAPINFO, BITMAPINFOHEADER, GetDIBits, DIB_RGB_COLORS, BI_RGB
};
use crate::ipc_types::MonitorCaptureInfo;

pub fn capture_rect(x: i32, y: i32, width: i32, height: i32) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, String> {
    unsafe {
        let hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            return Err("Failed to get screen DC".to_string());
        }
        
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
        if hdc_mem.is_invalid() {
            ReleaseDC(None, hdc_screen);
            return Err("Failed to create memory DC".to_string());
        }
        
        let hbm_screen = CreateCompatibleBitmap(hdc_screen, width, height);
        if hbm_screen.is_invalid() {
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);
            return Err("Failed to create compatible bitmap".to_string());
        }
        
        let old_obj = SelectObject(hdc_mem, hbm_screen.into());
        
        let res = BitBlt(
            hdc_mem,
            0,
            0,
            width,
            height,
            Some(hdc_screen),
            x,
            y,
            SRCCOPY,
        );
        
        if let Err(e) = res {
            SelectObject(hdc_mem, old_obj);
            let _ = DeleteObject(hbm_screen.into());
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);
            return Err(format!("BitBlt failed: {}", e));
        }
        
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (width * height * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: Default::default(),
        };
        
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        
        let lines_copied = GetDIBits(
            hdc_screen,
            hbm_screen,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        
        SelectObject(hdc_mem, old_obj);
        let _ = DeleteObject(hbm_screen.into());
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(None, hdc_screen);
        
        if lines_copied == 0 {
            return Err("GetDIBits failed".to_string());
        }
        
        for chunk in pixels.chunks_exact_mut(4) {
            let b = chunk[0];
            let r = chunk[2];
            chunk[0] = r;
            chunk[2] = b;
            chunk[3] = 255; // opaque alpha
        }
        
        ImageBuffer::from_raw(width as u32, height as u32, pixels)
            .ok_or_else(|| "Failed to create ImageBuffer".to_string())
    }
}

pub fn capture_all_monitors(app: &tauri::AppHandle) -> Result<Vec<MonitorCaptureInfo>, String> {
    let window = app.get_webview_window("main")
        .or_else(|| app.webview_windows().values().next().cloned())
        .ok_or_else(|| "No window found to query monitors".to_string())?;
        
    let monitors = window.available_monitors().map_err(|e| e.to_string())?;
    
    let temp_dir = std::env::temp_dir().join("JelloCapture");
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    
    let mut infos = Vec::new();
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
        
    for (i, m) in monitors.iter().enumerate() {
        let name = m.name().map(|s| s.as_str()).unwrap_or("Monitor").to_string();
        let size = m.size();
        let pos = m.position();
        let scale = m.scale_factor();
        
        let img = capture_rect(pos.x, pos.y, size.width as i32, size.height as i32)?;
        let file_name = format!("monitor_{}_{}.png", i, now_millis);
        let file_path = temp_dir.join(file_name);
        img.save(&file_path).map_err(|e| e.to_string())?;
        
        infos.push(MonitorCaptureInfo {
            index: i,
            name,
            x: pos.x,
            y: pos.y,
            width: size.width as i32,
            height: size.height as i32,
            scale_factor: scale,
            image_path: file_path.to_string_lossy().to_string(),
        });
    }
    
    Ok(infos)
}
