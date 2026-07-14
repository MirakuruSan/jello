use windows::Storage::Streams::{InMemoryRandomAccessStream, DataWriter};
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;

pub fn run_ocr_on_image(image_path: &str) -> Result<String, String> {
    let bytes = std::fs::read(image_path).map_err(|e| e.to_string())?;
    
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| format!("Failed to create InMemoryRandomAccessStream: {}", e))?;
        
    let writer = DataWriter::CreateDataWriter(&stream)
        .map_err(|e| format!("Failed to create DataWriter: {}", e))?;
        
    writer.WriteBytes(&bytes)
        .map_err(|e| format!("Failed to write bytes: {}", e))?;
        
    writer.StoreAsync().map_err(|e| e.to_string())?.get()
        .map_err(|e| format!("Failed to store bytes: {}", e))?;
        
    writer.FlushAsync().map_err(|e| e.to_string())?.get()
        .map_err(|e| format!("Failed to flush stream: {}", e))?;
        
    stream.Seek(0)
        .map_err(|e| format!("Failed to seek stream: {}", e))?;
        
    let decoder = BitmapDecoder::CreateAsync(&stream).map_err(|e| e.to_string())?.get()
        .map_err(|e| format!("Failed to create BitmapDecoder: {}", e))?;
        
    let software_bitmap = decoder.GetSoftwareBitmapAsync().map_err(|e| e.to_string())?.get()
        .map_err(|e| format!("Failed to decode SoftwareBitmap: {}", e))?;
        
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("Failed to create OcrEngine. Ensure OCR languages are installed on Windows: {}", e))?;
        
    let ocr_result = engine.RecognizeAsync(&software_bitmap).map_err(|e| e.to_string())?.get()
        .map_err(|e| format!("Failed to recognize text: {}", e))?;
        
    let lines_collection = ocr_result.Lines()
        .map_err(|e| format!("Failed to get lines: {}", e))?;
        
    let mut final_text = String::new();
    let mut pending_hyphen = false;
    
    let count = lines_collection.Size().unwrap_or(0);
    for i in 0..count {
        let line = lines_collection.GetAt(i)
            .map_err(|e| format!("Failed to get line {}: {}", i, e))?;
            
        let mut line_text = line.Text()
            .map_err(|e| format!("Failed to get text for line {}: {}", i, e))?
            .to_string();
            
        line_text = line_text.trim().to_string();
        
        if pending_hyphen {
            final_text.push_str(&line_text);
            pending_hyphen = false;
        } else {
            if !final_text.is_empty() {
                final_text.push_str("\r\n");
            }
            final_text.push_str(&line_text);
        }
        
        if line_text.ends_with('-') && i + 1 < count {
            final_text.pop(); // remove '-'
            pending_hyphen = true;
        }
    }
    
    Ok(final_text)
}
