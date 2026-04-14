/// .docx (Office Open XML) Exporter for Smart Office.
///
/// Implements a minimal zip container and XML generators to create a
/// valid `.docx` file in a `#![no_std]` environment without large
/// dependencies.

/// A very simple uncompressed ZIP file creator.
pub struct ZipWriter<'a> {
    buffer: &'a mut [u8],
    offset: usize,
    central_directory: [u8; 4096],
    cd_offset: usize,
    file_count: u16,
}

impl<'a> ZipWriter<'a> {
    pub fn new(buffer: &'a mut [u8]) -> Self {
        Self {
            buffer,
            offset: 0,
            central_directory: [0; 4096],
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

/// Generates a .docx file containing the given text.
pub fn generate_docx(text: &[u8], out_buffer: &mut [u8]) -> usize {
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
    // We dynamically build this based on the text.
    let mut doc_xml = [0u8; 16384]; // 16KB XML buffer
    let mut offset = 0;
    
    let header = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>"#;
    doc_xml[offset..offset + header.len()].copy_from_slice(header);
    offset += header.len();

    // Iterate through lines and create paragraphs
    let mut line_start = 0;
    for i in 0..=text.len() {
        if i == text.len() || text[i] == b'\n' {
            let p_start = br#"<w:p><w:r><w:t xml:space="preserve">"#;
            if offset + p_start.len() < doc_xml.len() {
                doc_xml[offset..offset + p_start.len()].copy_from_slice(p_start);
                offset += p_start.len();
            }

            let len = i - line_start;
            if len > 0 && offset + len < doc_xml.len() {
                // Minimal XML escaping could go here (e.g. '&' -> '&amp;').
                // For MVP, we copy raw.
                doc_xml[offset..offset + len].copy_from_slice(&text[line_start..i]);
                offset += len;
            }

            let p_end = br#"</w:t></w:r></w:p>"#;
            if offset + p_end.len() < doc_xml.len() {
                doc_xml[offset..offset + p_end.len()].copy_from_slice(p_end);
                offset += p_end.len();
            }

            line_start = i + 1;
        }
    }

    let footer = br#"</w:body></w:document>"#;
    if offset + footer.len() < doc_xml.len() {
        doc_xml[offset..offset + footer.len()].copy_from_slice(footer);
        offset += footer.len();
    }

    zip.add_file("word/document.xml", &doc_xml[..offset]);

    zip.finish()
}
