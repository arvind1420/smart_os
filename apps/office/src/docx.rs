/// .docx (Office Open XML) Exporter and Importer for Smart Office.
///
/// Implements a minimal zip container and XML generators/parsers to create and read
/// valid `.docx` files in a `#![no_std]` environment without large dependencies.

use crate::font::{TextSpan, TextStyle};

/// A very simple uncompressed ZIP file creator.
pub struct ZipWriter<'a> {
    buffer: &'a mut [u8],
    offset: usize,
    central_directory: [u8; 8192],
    cd_offset: usize,
    file_count: u16,
}

impl<'a> ZipWriter<'a> {
    pub fn new(buffer: &'a mut [u8]) -> Self {
        Self {
            buffer,
            offset: 0,
            central_directory: [0; 8192],
            cd_offset: 0,
            file_count: 0,
        }
    }

    /// Adds an uncompressed file to the ZIP.
    pub fn add_file(&mut self, filename: &str, data: &[u8]) {
        let name_bytes = filename.as_bytes();
        let name_len = name_bytes.len() as u16;
        let data_len = data.len() as u32;

        let local_header_offset = self.offset as u32;

        // Local file header (30 bytes)
        let header = &mut self.buffer[self.offset..self.offset + 30];
        header[0..4].copy_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // Signature
        header[4..6].copy_from_slice(&[10, 0]); // Version needed
        header[6..8].copy_from_slice(&[0, 0]); // General purpose bit flag
        header[8..10].copy_from_slice(&[0, 0]); // Compression (0 = Stored)
        header[10..14].copy_from_slice(&[0, 0, 0, 0]); // Modification time/date
        header[14..18].copy_from_slice(&[0; 4]); // CRC-32 (dummy 0 for MVP)
        header[18..22].copy_from_slice(&data_len.to_le_bytes()); // Compressed size
        header[22..26].copy_from_slice(&data_len.to_le_bytes()); // Uncompressed size
        header[26..28].copy_from_slice(&name_len.to_le_bytes()); // Filename length
        header[28..30].copy_from_slice(&[0, 0]); // Extra field length
        self.offset += 30;

        // Filename
        self.buffer[self.offset..self.offset + name_len as usize].copy_from_slice(name_bytes);
        self.offset += name_len as usize;

        // File Data
        self.buffer[self.offset..self.offset + data.len()].copy_from_slice(data);
        self.offset += data.len();

        // Central Directory Record (46 bytes)
        let cd = &mut self.central_directory[self.cd_offset..self.cd_offset + 46];
        cd[0..4].copy_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // Signature
        cd[4..6].copy_from_slice(&[20, 0]); // Version made by
        cd[6..8].copy_from_slice(&[10, 0]); // Version needed
        cd[8..10].copy_from_slice(&[0, 0]); // Flags
        cd[10..12].copy_from_slice(&[0, 0]); // Compression
        cd[12..16].copy_from_slice(&[0, 0, 0, 0]); // Mod time
        cd[16..20].copy_from_slice(&[0; 4]); // CRC-32
        cd[20..24].copy_from_slice(&data_len.to_le_bytes()); // Compressed size
        cd[24..28].copy_from_slice(&data_len.to_le_bytes()); // Uncompressed size
        cd[28..30].copy_from_slice(&name_len.to_le_bytes()); // Filename len
        cd[30..42].fill(0); // Extra field len, Comment len, Disk start, Internal attr, External attr
        cd[42..46].copy_from_slice(&local_header_offset.to_le_bytes()); // Local header offset
        self.cd_offset += 46;

        self.central_directory[self.cd_offset..self.cd_offset + name_len as usize].copy_from_slice(name_bytes);
        self.cd_offset += name_len as usize;

        self.file_count += 1;
    }

    /// Finalizes the ZIP and returns the total size.
    pub fn finish(mut self) -> usize {
        let cd_start = self.offset as u32;
        let cd_size = self.cd_offset as u32;

        // Write central directory
        self.buffer[self.offset..self.offset + self.cd_offset]
            .copy_from_slice(&self.central_directory[..self.cd_offset]);
        self.offset += self.cd_offset;

        // End of central directory record (22 bytes)
        let eocd = &mut self.buffer[self.offset..self.offset + 22];
        eocd[0..4].copy_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
        eocd[4..8].fill(0); // Disk numbers
        eocd[8..10].copy_from_slice(&self.file_count.to_le_bytes()); // Records on this disk
        eocd[10..12].copy_from_slice(&self.file_count.to_le_bytes()); // Total records
        eocd[12..16].copy_from_slice(&cd_size.to_le_bytes()); // Size of CD
        eocd[16..20].copy_from_slice(&cd_start.to_le_bytes()); // Offset of CD
        eocd[20..22].copy_from_slice(&[0, 0]); // Comment length

        self.offset + 22
    }
}

/// Generates a .docx file containing the given text and formatting spans.
pub fn generate_docx(text: &[u8], spans: &[TextSpan], span_count: usize, out_buffer: &mut [u8]) -> usize {
    let mut zip = ZipWriter::new(out_buffer);

    // 1. [Content_Types].xml
    let content_types = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
    zip.add_file("[Content_Types].xml", content_types);

    // 2. _rels/.rels
    let rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
    zip.add_file("_rels/.rels", rels);

    // 3. word/document.xml
    let mut doc_xml = alloc::vec::Vec::new();
    doc_xml.extend_from_slice(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>"#);

    // Iterate through text lines and construct paragraph blocks
    let mut line_start = 0;
    for i in 0..=text.len() {
        if i == text.len() || text[i] == b'\n' {
            // Write paragraph start tag
            doc_xml.extend_from_slice(br#"<w:p>"#);

            // Output runs of text inside the paragraph matching active spans
            let line_end = i;
            let mut current_pos = line_start;

            while current_pos < line_end {
                // Find matching span
                let mut style = TextStyle::default();
                let mut span_end = line_end;

                for s in 0..span_count {
                    if current_pos >= spans[s].start && current_pos < spans[s].end {
                        style = spans[s].style;
                        span_end = span_end.min(spans[s].end);
                        break;
                    }
                }

                let run_len = span_end - current_pos;
                if run_len > 0 {
                    doc_xml.extend_from_slice(br#"<w:r>"#);
                    
                    // Render Run Properties
                    if style.bold || style.italic || style.size != 16 {
                        doc_xml.extend_from_slice(br#"<w:rPr>"#);
                        if style.bold {
                            doc_xml.extend_from_slice(br#"<w:b/>"#);
                        }
                        if style.italic {
                            doc_xml.extend_from_slice(br#"<w:i/>"#);
                        }
                        if style.size != 16 {
                            // w:sz val is in half-points (e.g. 16pt -> 32)
                            let sz_val = style.size * 2;
                            doc_xml.extend_from_slice(br#"<w:sz w:val=""#);
                            // Simple integer to string conversion
                            let mut num_buf = [0u8; 10];
                            let num_str = u16_to_str(sz_val, &mut num_buf);
                            doc_xml.extend_from_slice(num_str.as_bytes());
                            doc_xml.extend_from_slice(br#"" />"#);
                        }
                        doc_xml.extend_from_slice(br#"</w:rPr>"#);
                    }

                    // Text tag
                    doc_xml.extend_from_slice(br#"<w:t xml:space="preserve">"#);
                    
                    // Copy run text with XML escaping
                    for &b in &text[current_pos..span_end] {
                        match b {
                            b'&' => doc_xml.extend_from_slice(br#"&amp;"#),
                            b'<' => doc_xml.extend_from_slice(br#"&lt;"#),
                            b'>' => doc_xml.extend_from_slice(br#"&gt;"#),
                            _ => doc_xml.push(b),
                        }
                    }

                    doc_xml.extend_from_slice(br#"</w:t></w:r>"#);
                }

                current_pos = span_end;
            }

            // Write paragraph end tag
            doc_xml.extend_from_slice(br#"</w:p>"#);
            line_start = i + 1;
        }
    }

    doc_xml.extend_from_slice(br#"</w:body></w:document>"#);
    zip.add_file("word/document.xml", &doc_xml);

    zip.finish()
}

fn u16_to_str(mut val: u16, buf: &mut [u8]) -> &str {
    if val == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut i = buf.len();
    while val > 0 {
        i -= 1;
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}

/// Locate standard word/document.xml in stored ZIP archive
pub fn find_xml_in_zip(zip_bytes: &[u8]) -> Option<&[u8]> {
    let mut i = 0;
    while i + 30 <= zip_bytes.len() {
        if zip_bytes[i..i+4] == [0x50, 0x4B, 0x03, 0x04] {
            let data_size = u32::from_le_bytes([zip_bytes[i+18], zip_bytes[i+19], zip_bytes[i+20], zip_bytes[i+21]]) as usize;
            let name_len = u16::from_le_bytes([zip_bytes[i+26], zip_bytes[i+27]]) as usize;
            let extra_len = u16::from_le_bytes([zip_bytes[i+28], zip_bytes[i+29]]) as usize;
            
            if i + 30 + name_len <= zip_bytes.len() {
                let filename = &zip_bytes[i+30..i+30+name_len];
                if filename == b"word/document.xml" {
                    let data_start = i + 30 + name_len + extra_len;
                    if data_start + data_size <= zip_bytes.len() {
                        return Some(&zip_bytes[data_start..data_start + data_size]);
                    }
                }
            }
            i += 30 + name_len + extra_len + data_size;
        } else {
            i += 1;
        }
    }
    None
}

/// Parses the word/document.xml content to reconstruct text and styled spans
pub fn parse_docx(
    zip_bytes: &[u8],
    out_text: &mut alloc::vec::Vec<u8>,
    out_spans: &mut alloc::vec::Vec<TextSpan>,
) -> Result<(), &'static str> {
    let xml = find_xml_in_zip(zip_bytes).ok_or("Failed to find word/document.xml in docx archive")?;
    
    let mut i = 0;
    let mut bold = false;
    let mut italic = false;
    let mut size = 16u16;
    
    while i < xml.len() {
        if xml[i] == b'<' {
            let mut end_tag = i;
            while end_tag < xml.len() && xml[end_tag] != b'>' {
                end_tag += 1;
            }
            if end_tag >= xml.len() { break; }
            
            let tag_content = &xml[i+1..end_tag];
            
            if tag_content == b"w:p" {
                // New paragraph starts
            } else if tag_content == b"/w:p" {
                out_text.push(b'\n');
            } else if tag_content == b"w:r" {
                // Reset styles to default
                bold = false;
                italic = false;
                size = 16;
            } else if tag_content == b"w:b/" || tag_content.starts_with(b"w:b ") {
                bold = true;
            } else if tag_content == b"w:i/" || tag_content.starts_with(b"w:i ") {
                italic = true;
            } else if tag_content.starts_with(b"w:sz ") {
                // Parse font size (e.g. w:val="32")
                let mut sz_val_opt = None;
                for idx in 0..tag_content.len() {
                    if tag_content[idx..].starts_with(b"w:val=\"") {
                        let start = idx + 7;
                        let mut end = start;
                        while end < tag_content.len() && tag_content[end] >= b'0' && tag_content[end] <= b'9' {
                            end += 1;
                        }
                        if let Ok(s) = core::str::from_utf8(&tag_content[start..end]) {
                            if let Ok(val) = s.parse::<u16>() {
                                sz_val_opt = Some(val);
                            }
                        }
                        break;
                    }
                }
                if let Some(val) = sz_val_opt {
                    size = val / 2;
                }
            } else if tag_content.starts_with(b"w:t") {
                let text_start = end_tag + 1;
                let mut text_end = text_start;
                while text_end + 5 <= xml.len() && &xml[text_end..text_end+6] != b"</w:t>" {
                    text_end += 1;
                }
                
                if text_end + 5 <= xml.len() {
                    let raw_text = &xml[text_start..text_end];
                    let mut unescaped = alloc::vec::Vec::new();
                    let mut ti = 0;
                    while ti < raw_text.len() {
                        if raw_text[ti] == b'&' {
                            if ti + 4 <= raw_text.len() && &raw_text[ti..ti+5] == b"&amp;" {
                                unescaped.push(b'&');
                                ti += 5;
                            } else if ti + 3 <= raw_text.len() && &raw_text[ti..ti+4] == b"&lt;" {
                                unescaped.push(b'<');
                                ti += 4;
                            } else if ti + 3 <= raw_text.len() && &raw_text[ti..ti+4] == b"&gt;" {
                                unescaped.push(b'>');
                                ti += 4;
                            } else {
                                unescaped.push(raw_text[ti]);
                                ti += 1;
                            }
                        } else {
                            unescaped.push(raw_text[ti]);
                            ti += 1;
                        }
                    }
                    
                    if !unescaped.is_empty() {
                        let span_start = out_text.len();
                        out_text.extend_from_slice(&unescaped);
                        let span_end = out_text.len();
                        out_spans.push(TextSpan {
                            start: span_start,
                            end: span_end,
                            style: TextStyle { bold, italic, size },
                        });
                    }
                    i = text_end + 6;
                    continue;
                }
            }
            i = end_tag + 1;
        } else {
            i += 1;
        }
    }
    Ok(())
}
