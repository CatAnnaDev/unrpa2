use eframe::egui;
use flate2::write::ZlibEncoder;
use flate2::{read::ZlibDecoder, Compression};
use rodio::{Decoder, OutputStream, Sink};
use serde_pickle::{DeOptions, Value};
use std::collections::HashMap;
use std::fs::{create_dir_all, File};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Debug, Clone)]
struct RpaFileEntry {
    offset: u64,
    length: u64,
    prefix: Vec<u8>,
    data: Option<Vec<u8>>,
    modified: bool,
    to_delete: bool,
}

#[derive(Debug, Clone)]
struct BackupEntry {
    filename: String,
    data: Vec<u8>,
    timestamp: chrono::DateTime<chrono::Utc>,
}

struct RpaEditor {
    version: f32,
    key: u32,
    indexes: HashMap<String, RpaFileEntry>,
    archive_path: Option<String>,
    modified: bool,

    selected_file: Option<String>,
    preview_data: Option<Vec<u8>>,
    preview_image: Option<egui::ColorImage>,
    preview_text: Option<String>,
    search_filter: String,
    show_add_dialog: bool,
    add_file_path: String,
    add_file_name: String,
    status_message: String,

    file_to_preview: Option<String>,
    file_to_remove: Option<String>,
    file_to_replace: Option<(String, String)>,
    batch_replace_to_execute: Option<String>,

    show_dump_dialog: bool,

    show_backup_dialog: bool,
    backup_history: Vec<BackupEntry>,
    show_batch_replace_dialog: bool,
    batch_replace_folder: String,
    show_statistics_dialog: bool,
    auto_backup: bool,
    compression_level: u32,

    filter_type: String,
    sort_by: String,
    sort_ascending: bool,

    image_zoom: f32,
    hex_view_offset: usize,

    audio_player: AudioPlayer,
    is_playing: bool,
}

impl Default for RpaEditor {
    fn default() -> Self {
        Self {
            version: 3.2,
            key: 0xDEADBEEF,
            indexes: HashMap::new(),
            archive_path: None,
            modified: false,
            selected_file: None,
            preview_data: None,
            preview_image: None,
            preview_text: None,
            search_filter: String::new(),
            show_add_dialog: false,
            add_file_path: String::new(),
            add_file_name: String::new(),
            status_message: "Ready".to_string(),
            file_to_preview: None,
            file_to_remove: None,
            file_to_replace: None,
            batch_replace_to_execute: None,
            show_dump_dialog: false,

            show_backup_dialog: false,
            backup_history: Vec::new(),
            show_batch_replace_dialog: false,
            batch_replace_folder: String::new(),
            show_statistics_dialog: false,
            auto_backup: true,
            compression_level: 6,

            filter_type: "all".to_string(),
            sort_by: "name".to_string(),
            sort_ascending: true,

            image_zoom: 1.0,
            hex_view_offset: 0,
            audio_player: AudioPlayer::new(),
            is_playing: false,
        }
    }
}

impl RpaEditor {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    fn load_rpa(&mut self, path: &str) -> anyhow::Result<()> {
        let mut file = File::open(path)?;

        self.version = self.get_version(&mut file)?;

        self.indexes = self.extract_indexes(&mut file)?;
        self.archive_path = Some(path.to_string());
        self.modified = false;

        self.selected_file = None;
        self.preview_data = None;
        self.preview_image = None;
        self.preview_text = None;

        self.status_message = format!("Loaded {} files from {}", self.indexes.len(), path);
        Ok(())
    }

    fn load_entries_data(
        &self,
        index: &mut HashMap<String, RpaFileEntry>,
        file: &mut File,
    ) -> anyhow::Result<()> {
        for (filename, entry) in index.iter() {
            if entry.offset + entry.length > file.metadata()?.len() {
                println!("❌ ERREUR : dépassement du fichier !");
            }

            file.seek(SeekFrom::Start(entry.offset))?;
            let mut buffer = vec![0u8; entry.length as usize];
            match file.read_exact(&mut buffer) {
                Ok(_) => {}
                Err(e) => println!("❌ Lecture échouée: {filename} ({})", e),
            }
        }

        Ok(())
    }

    fn parse_index_pickle(&self, data: &[u8]) -> anyhow::Result<HashMap<String, RpaFileEntry>> {
        let value: Value = serde_pickle::value_from_slice(data, DeOptions::new().decode_strings())?;

        let mut indexes = HashMap::new();

        if let Value::Dict(dict) = value {
            for (key, val) in dict {
                let filename = key.to_string().replace("\"", "");

                if let Value::List(list) = val {
                    if list.len() == 1 {
                        if let Value::Tuple(tuple) = &list[0] {
                            if tuple.len() == 3 {
                                if let (
                                    Value::I64(offset),
                                    Value::I64(length),
                                    Value::Bytes(prefix),
                                ) = (&tuple[0], &tuple[1], &tuple[2])
                                {
                                    let offset = *offset as u64 ^ self.key as u64;
                                    let length = *length as u64 ^ self.key as u64;
                                    indexes.insert(
                                        filename.clone(),
                                        RpaFileEntry {
                                            offset,
                                            length,
                                            prefix: prefix.clone(),
                                            data: None,
                                            modified: false,
                                            to_delete: false,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }

            Ok(indexes)
        } else {
            Err(anyhow::anyhow!("Pickle root is not a dict"))
        }
    }

    fn get_version(&self, file: &mut File) -> anyhow::Result<f32> {
        file.seek(SeekFrom::Start(0))?;
        let mut buffer = vec![0u8; 32];
        file.read_exact(&mut buffer)?;

        let header = String::from_utf8_lossy(&buffer);

        if header.starts_with("RPA-3.2 ") {
            Ok(3.2)
        } else if header.starts_with("RPA-3.0 ") {
            Ok(3.0)
        } else if header.starts_with("RPA-2") {
            Ok(2.0)
        } else {
            Err(anyhow::anyhow!("Unsupported RPA version"))
        }
    }

    fn extract_indexes(
        &mut self,
        file: &mut File,
    ) -> anyhow::Result<HashMap<String, RpaFileEntry>> {
        file.seek(SeekFrom::Start(0))?;
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while file.read_exact(&mut byte).is_ok() && byte[0] != b'\n' {
            line.push(byte[0]);
        }

        let header_line = String::from_utf8_lossy(&line);
        let parts: Vec<&str> = header_line.split_whitespace().collect();

        let offset = u64::from_str_radix(parts[1], 16)?;

        if self.version >= 3.0 {
            self.key = 0;
            let key_start = if self.version >= 3.2 { 3 } else { 2 };
            for &key_part in &parts[key_start..] {
                let subkey = u32::from_str_radix(key_part, 16)?;
                self.key ^= subkey;
            }
        }

        file.seek(SeekFrom::Start(offset))?;
        let mut compressed_data = Vec::new();
        file.read_to_end(&mut compressed_data)?;

        let mut decoder = ZlibDecoder::new(&compressed_data[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;

        match self.parse_index_pickle(&decompressed) {
            Ok(mut indexes) => {
                self.load_entries_data(&mut indexes, file)?;
                Ok(indexes)
            }
            Err(e) => {
                eprintln!("⚠️ Erreur pickle: {e}, on tente l'extraction heuristique...");
                self.parse_binary_dict(&decompressed)
            }
        }
    }

    fn parse_binary_dict(&self, data: &[u8]) -> anyhow::Result<HashMap<String, RpaFileEntry>> {
        let mut indexes = HashMap::new();
        let mut pos = 0;

        while pos < data.len() {
            if let Some((filename, filename_end)) = self.extract_filename_at_pos(data, pos) {
                if let Some(entry) = self.find_entry_data_after_filename(data, filename_end) {
                    indexes.insert(filename, entry);
                    pos = filename_end + 50;
                } else {
                    pos = filename_end;
                }
            } else {
                pos += 1;
            }
        }

        Ok(indexes)
    }

    fn extract_filename_at_pos(&self, data: &[u8], start_pos: usize) -> Option<(String, usize)> {
        let mut pos = start_pos;

        while pos < data.len() {
            if data[pos].is_ascii_graphic() || data[pos] == b'/' {
                break;
            }
            pos += 1;
        }

        if pos >= data.len() {
            return None;
        }

        let filename_start = pos;

        let is_valid_char = |c: u8| {
            c.is_ascii() && (c as char).is_ascii_graphic() && !"\"\\:*?<>|".contains(c as char)
        };

        while pos < data.len() && is_valid_char(data[pos]) {
            pos += 1;
        }

        let slice = &data[filename_start..pos];

        if let Ok(filename) = std::str::from_utf8(slice) {
            if self.is_valid_filename(filename) {
                return Some((filename.to_string(), pos));
            }
        }

        None
    }

    fn find_entry_data_after_filename(
        &self,
        data: &[u8],
        start_pos: usize,
    ) -> Option<RpaFileEntry> {
        let search_end = std::cmp::min(start_pos + 100, data.len());

        for pos in start_pos..search_end {
            if pos + 10 < data.len() && data[pos] == b'J' {
                if let Some((offset, length, prefix)) = self.extract_j_values_at(data, pos) {
                    if self.is_reasonable_entry(offset, length) {
                        return Some(RpaFileEntry {
                            offset,
                            length,
                            prefix,
                            data: None,
                            modified: false,
                            to_delete: false,
                        });
                    }
                }
            }
        }

        None
    }

    fn extract_j_values_at(&self, data: &[u8], pos: usize) -> Option<(u64, u64, Vec<u8>)> {
        if pos + 9 < data.len() && data[pos] == b'J' {
            let val1_bytes = [data[pos + 1], data[pos + 2], data[pos + 3], data[pos + 4]];
            let val1 = u32::from_le_bytes(val1_bytes);

            for next_pos in (pos + 5)..(pos + 15) {
                if next_pos + 4 < data.len() && data[next_pos] == b'J' {
                    let val2_bytes = [
                        data[next_pos + 1],
                        data[next_pos + 2],
                        data[next_pos + 3],
                        data[next_pos + 4],
                    ];
                    let val2 = u32::from_le_bytes(val2_bytes);

                    let offset = (val1 ^ self.key) as u64;
                    let length = (val2 ^ self.key) as u64;

                    if self.is_reasonable_entry(offset, length) {
                        return Some((offset, length, Vec::new()));
                    }
                }
            }
        }

        None
    }

    fn is_valid_filename(&self, filename: &str) -> bool {
        if filename.len() < 2 || filename.len() > 200 {
            return false;
        }

        let extensions = [
            ".png", ".jpg", ".jpeg", ".webp", ".webm", ".avi", ".mp4", ".mov", ".ogg", ".wav",
            ".mp3", ".flac", ".rpy", ".rpyc",
        ];

        extensions.iter().any(|&ext| filename.ends_with(ext))
    }

    fn is_reasonable_entry(&self, offset: u64, length: u64) -> bool {
        offset > 50
            && offset < 2_000_000_000
            && length > 0
            && length < 500_000_000
            && offset + length < 2_000_000_000
    }

    fn load_file_data(&self, filename: &str) -> anyhow::Result<Vec<u8>> {
        if let Some(entry) = self.indexes.get(filename) {
            if let Some(ref data) = entry.data {
                return Ok(data.clone());
            }

            if let Some(ref archive_path) = self.archive_path {
                let mut file = File::open(archive_path)?;
                file.seek(SeekFrom::Start(entry.offset))?;

                let mut content = Vec::new();
                content.extend_from_slice(&entry.prefix);

                let remaining_length = entry.length - entry.prefix.len() as u64;
                let mut buffer = vec![0u8; remaining_length as usize];
                file.read_exact(&mut buffer)?;
                content.extend_from_slice(&buffer);

                return Ok(content);
            }
        }

        Err(anyhow::anyhow!("File not found"))
    }

    fn decompile_rpyc(&self, data: &[u8]) -> Option<String> {
        if data.len() < 16 {
            return None;
        }

        let mut result = String::new();
        result.push_str("# Decompiled .rpyc file\n");
        result.push_str("# Enhanced decompilation with pattern recognition\n\n");

        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        result.push_str(&format!("# Python bytecode magic: 0x{:08X}\n", magic));

        let mut pos = 16;
        let mut found_strings = Vec::new();
        let mut labels = Vec::new();
        let mut characters = Vec::new();

        while pos < data.len() {
            if pos + 4 < data.len() {
                let str_len =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize;

                if str_len > 0 && str_len < 1000 && pos + 4 + str_len < data.len() {
                    if let Ok(s) = String::from_utf8(data[pos + 4..pos + 4 + str_len].to_vec()) {
                        if s.len() > 2
                            && s.chars().all(|c| c.is_ascii_graphic() || c.is_whitespace())
                        {
                            if s.starts_with("label_") || s.starts_with("scene_") {
                                labels.push(s.clone());
                            } else if s.len() < 30
                                && s.chars().any(|c| c.is_alphabetic())
                                && !s.contains(' ')
                            {
                                characters.push(s.clone());
                            } else {
                                found_strings.push(s);
                            }
                            pos += 4 + str_len;
                            continue;
                        }
                    }
                }
            }
            pos += 1;
        }

        if !labels.is_empty() {
            result.push_str("\n# === LABELS DETECTED ===\n");
            for label in &labels {
                result.push_str(&format!("label {}:\n    # [content]\n\n", label));
            }
        }

        if !characters.is_empty() {
            result.push_str("\n# === CHARACTERS DETECTED ===\n");
            for character in &characters {
                result.push_str(&format!(
                    "define {} = Character(\"{}\")\n",
                    character, character
                ));
            }
        }

        if !found_strings.is_empty() {
            result.push_str("\n# === DIALOGUE/TEXT STRINGS ===\n");
            for (i, s) in found_strings.iter().enumerate() {
                if s.len() > 10 && s.contains(' ') {
                    result.push_str(&format!("# Text {}: \"{}\"\n", i, s));
                }
            }
        }

        Some(result)
    }

    fn preview_file(&mut self, filename: &str) {
        if let Ok(data) = self.load_file_data(filename) {
            self.preview_data = Some(data.clone());
            self.preview_image = None;
            self.preview_text = None;
            self.image_zoom = 1.0;
            self.hex_view_offset = 0;

            let lower = filename.to_lowercase();

            if lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".webp")
            {
                if let Ok(img) = image::load_from_memory(&data) {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                    self.preview_image = Some(color_image);
                    self.status_message = format!(
                        "Loaded image: {}×{} ({:.1} KB)",
                        rgba.width(),
                        rgba.height(),
                        data.len() as f32 / 1024.0
                    );
                } else {
                    self.status_message = "Failed to load image".to_string();
                }
            } else if lower.ends_with(".rpyc") {
                if let Some(decompiled) = self.decompile_rpyc(&data) {
                    self.preview_text = Some(decompiled);
                    self.status_message = "Decompiled .rpyc file (enhanced extraction)".to_string();
                } else {
                    self.status_message = "Could not decompile .rpyc file".to_string();
                }
            } else if lower.ends_with(".rpy") {
                if let Ok(text) = String::from_utf8(data.clone()) {
                    self.preview_text = Some(text);
                    self.status_message = "Loaded Ren'Py script".to_string();
                } else {
                    self.status_message = "Could not decode a text file".to_string();
                }
            } else {
                let info = self.generate_media_info(filename, &data);
                self.preview_text = Some(info);
                self.status_message =
                    format!("Loaded {} ({:.1} KB)", filename, data.len() as f32 / 1024.0);
            }
        }
    }

    fn generate_media_info(&self, filename: &str, data: &[u8]) -> String {
        let lower = filename.to_lowercase();
        let mut info = String::new();

        if lower.ends_with(".webm") || lower.ends_with(".mp4") || lower.ends_with(".avi") {
            info.push_str("🎬 Video File Analysis\n");
            info.push_str("═══════════════════════\n\n");
        } else if lower.ends_with(".ogg") || lower.ends_with(".wav") || lower.ends_with(".mp3") {
            info.push_str("🎵 Audio File Analysis\n");
            info.push_str("═══════════════════════\n\n");
        } else {
            info.push_str("📄 File Analysis\n");
            info.push_str("═══════════════\n\n");
        }

        info.push_str(&format!("📁 Filename: {}\n", filename));
        info.push_str(&format!(
            "📊 Size: {} ({} bytes)\n",
            Self::format_bytes(data.len() as u64),
            data.len()
        ));

        if lower.ends_with(".webm") && data.len() > 20 {
            info.push_str("🎬 Format: WebM (VP8/VP9)\n");
            if data[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
                info.push_str("✅ Valid EBML header detected\n");
                info.push_str("📹 Container: Matroska-based\n");
            }
        } else if lower.ends_with(".mp4") && data.len() > 20 {
            info.push_str("🎬 Format: MP4 (H.264/H.265)\n");
            if &data[4..8] == b"ftyp" {
                info.push_str("✅ Valid MP4 header detected\n");
                let brand = String::from_utf8_lossy(&data[8..12]);
                info.push_str(&format!("🏷️ Brand: {}\n", brand));
            }
        } else if lower.ends_with(".ogg") && data.len() > 4 {
            info.push_str("🎵 Format: OGG Vorbis\n");
            if &data[0..4] == b"OggS" {
                info.push_str("✅ Valid OGG header detected\n");
                info.push_str("🎶 Codec: Vorbis audio\n");
            }
        } else if lower.ends_with(".wav") && data.len() > 44 {
            info.push_str("🎵 Format: WAV (Uncompressed)\n");
            if &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
                info.push_str("✅ Valid WAV header detected\n");

                let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
                let byte_rate = u32::from_le_bytes([data[28], data[29], data[30], data[31]]);
                let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);
                let channels = u16::from_le_bytes([data[22], data[23]]);

                if sample_rate > 0 && byte_rate > 0 {
                    let duration = (data.len() as u32 - 44) / byte_rate;
                    info.push_str(&format!("🔊 Sample Rate: {} Hz\n", sample_rate));
                    info.push_str(&format!("🎚️ Channels: {}\n", channels));
                    info.push_str(&format!("📏 Bits: {} bit\n", bits_per_sample));
                    info.push_str(&format!("⏱️ Duration: ~{}s\n", duration));
                }
            }
        } else if lower.ends_with(".mp3") && data.len() > 10 {
            info.push_str("🎵 Format: MP3 (MPEG Audio)\n");
            if &data[0..3] == b"ID3" {
                info.push_str("✅ ID3 tags detected\n");
                let version = data[3];
                info.push_str(&format!("🏷️ ID3 Version: 2.{}\n", version));
            } else if data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
                info.push_str("✅ Valid MP3 frame header detected\n");
            }
        }

        info.push_str("\n💡 Usage Notes:\n");
        info.push_str("• Use 'Extract' to save the file\n");
        info.push_str("• Use 'Open Folder' to extract & view\n");
        if lower.ends_with(".ogg") || lower.ends_with(".wav") || lower.ends_with(".mp3") {
            info.push_str("• use play audio button\n");
        }

        if lower.ends_with(".webm") || lower.ends_with(".mp4") {
            info.push_str("• Media preview not available in editor")
        }

        info
    }

    fn replace_file(&mut self, filename: &str, new_file_path: &str) -> anyhow::Result<()> {
        println!(
            "🔄 Attempting to replace {} with {}",
            filename, new_file_path
        );

        let new_path = Path::new(filename);
        if !new_path.exists() {
            return Err(anyhow::anyhow!(
                "Replacement file isn't found: {}",
                filename
            ));
        }

        if !new_path.is_file() {
            return Err(anyhow::anyhow!("Path is not a file: {}", filename));
        }

        let new_data = std::fs::read(filename).map_err(|e| {
            anyhow::anyhow!("Failed to read replacement file '{}': {}", filename, e)
        })?;

        println!(
            "✅ Successfully read {} bytes from {}",
            new_data.len(),
            new_file_path
        );

        if let Some(entry) = self.indexes.get_mut(new_file_path) {
            entry.data = Some(new_data.clone());
            entry.modified = true;
            entry.length = new_data.len() as u64;
            self.modified = true;

            self.status_message = format!("Replaced: {} ({} bytes)", filename, new_data.len());

            println!(
                "✅ Replaced {} with {} ({} bytes)",
                filename,
                new_file_path,
                new_data.len()
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "File isn't found in the archive: {}",
                filename
            ))
        }
    }

    fn add_file(&mut self, file_path: &str, archive_name: &str) -> anyhow::Result<()> {
        let data = std::fs::read(file_path)?;

        if self.auto_backup && self.indexes.contains_key(archive_name) {
            if let Ok(old_data) = self.load_file_data(archive_name) {
                let backup = BackupEntry {
                    filename: archive_name.to_string(),
                    data: old_data,
                    timestamp: chrono::Utc::now(),
                };
                self.backup_history.push(backup);

                if self.backup_history.len() > 10 {
                    self.backup_history.remove(0);
                }
            }
        }

        let entry = RpaFileEntry {
            offset: 0,
            length: data.len() as u64,
            prefix: Vec::new(),
            data: Some(data),
            modified: true,
            to_delete: false,
        };

        let is_new = !self.indexes.contains_key(archive_name);
        self.indexes.insert(archive_name.to_string(), entry);
        self.modified = true;

        if is_new {
            self.status_message = format!(
                "Added {} ({} total files)",
                archive_name,
                self.indexes.len()
            );
        } else {
            self.status_message = format!(
                "Replaced {} ({} total files)",
                archive_name,
                self.indexes.len()
            );
        }

        Ok(())
    }

    fn remove_file(&mut self, filename: &str) {
        if let Some(entry) = self.indexes.get_mut(filename) {
            entry.to_delete = true;
            self.modified = true;
            self.status_message = format!("Marked {} for deletion", filename);
        }
    }

    //

    pub fn save_rpa(&self, archive_path: &str) -> anyhow::Result<()> {
        let old_data = std::fs::read(&self.archive_path.clone().unwrap())?;
        let mut offset = 0x34;
        let mut out = File::create(archive_path)?;

        out.seek(SeekFrom::Start(offset))?;

        let mut new_indexes = HashMap::new();

        let mut files: Vec<_> = self.indexes.iter().collect();
        files.sort_by_key(|(k, _)| *k);

        for (name, entry) in files {
            let data = if let Some(d) = &entry.data {
                d.clone()
            } else {
                let start = entry.offset as usize;
                let end = start + entry.length as usize;
                old_data
                    .get(start..end)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Data isn't found in the old archive for {name}")
                    })?
                    .to_vec()
            };

            out.write_all(&data)?;

            if self.version == 3.0 {
                new_indexes.insert(
                    name.clone(),
                    vec![(
                        offset ^ self.key as u64,
                        data.len() as u64 ^ self.key as u64,
                    )],
                );
            } else {
                new_indexes.insert(name.clone(), vec![(offset, data.len() as u64)]);
            }

            offset += data.len() as u64;
        }

        let raw_index = serde_pickle::to_vec(&new_indexes, Default::default())?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw_index)?;
        let compressed_index = encoder.finish()?;

        out.write_all(&compressed_index)?;

        out.seek(SeekFrom::Start(0))?;
        if self.version == 3.0 {
            write!(out, "RPA-3.0 {:016x} {:08x}\n", offset, self.key)?;
        } else {
            write!(out, "RPA-2.0 {:016x}\n", offset)?;
        }

        Ok(())
    }

    //

    fn format_bytes(bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
        let mut size = bytes as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        if unit_index == 0 {
            format!("{} {}", bytes, UNITS[unit_index])
        } else {
            format!("{:.1} {}", size, UNITS[unit_index])
        }
    }

    fn get_file_icon(filename: &str) -> &'static str {
        let lower = filename.to_lowercase();
        if lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".webp")
        {
            "🖼️"
        } else if lower.ends_with(".webm") || lower.ends_with(".mp4") || lower.ends_with(".avi") {
            "🎬"
        } else if lower.ends_with(".ogg") || lower.ends_with(".wav") || lower.ends_with(".mp3") {
            "🎵"
        } else if lower.ends_with(".rpy") || lower.ends_with(".rpyc") {
            "📜"
        } else {
            "📄"
        }
    }

    fn get_file_type_color(filename: &str) -> egui::Color32 {
        let lower = filename.to_lowercase();
        if lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".webp")
        {
            egui::Color32::from_rgb(100, 200, 100)
        } else if lower.ends_with(".webm") || lower.ends_with(".mp4") || lower.ends_with(".avi") {
            egui::Color32::from_rgb(200, 100, 100)
        } else if lower.ends_with(".ogg") || lower.ends_with(".wav") || lower.ends_with(".mp3") {
            egui::Color32::from_rgb(100, 100, 200)
        } else if lower.ends_with(".rpy") || lower.ends_with(".rpyc") {
            egui::Color32::from_rgb(200, 200, 100)
        } else {
            egui::Color32::GRAY
        }
    }

    fn get_file_type(&self, filename: &str) -> &'static str {
        let lower = filename.to_lowercase();
        if lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".webp")
        {
            "images"
        } else if lower.ends_with(".webm") || lower.ends_with(".mp4") || lower.ends_with(".avi") {
            "videos"
        } else if lower.ends_with(".ogg") || lower.ends_with(".wav") || lower.ends_with(".mp3") {
            "audio"
        } else if lower.ends_with(".rpy") || lower.ends_with(".rpyc") {
            "scripts"
        } else {
            "other"
        }
    }

    fn count_files_by_type(&self) -> HashMap<&'static str, usize> {
        let mut counts = HashMap::new();
        for filename in self.indexes.keys() {
            let file_type = self.get_file_type(filename);
            *counts.entry(file_type).or_insert(0) += 1;
        }
        counts
    }

    fn dump_files_by_type(&self, file_type: &str, base_path: &Path) -> anyhow::Result<usize> {
        let mut count = 0;

        let type_dir = base_path.join(file_type);
        create_dir_all(&type_dir)?;

        for (filename, entry) in &self.indexes {
            if entry.to_delete {
                continue;
            }

            let current_type = self.get_file_type(filename);
            if current_type == file_type || file_type == "all" {
                if let Ok(data) = self.load_file_data(filename) {
                    let file_path = if file_type == "all" {
                        let subdir = base_path.join(current_type);
                        create_dir_all(&subdir)?;
                        subdir.join(filename)
                    } else {
                        type_dir.join(filename)
                    };

                    if let Some(parent) = file_path.parent() {
                        create_dir_all(parent)?;
                    }

                    std::fs::write(&file_path, data)?;
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    fn dump_all_files(&self, base_path: &Path) -> anyhow::Result<usize> {
        self.dump_files_by_type("all", base_path)
    }

    fn get_filtered_sorted_files(&self) -> Vec<(&String, &RpaFileEntry)> {
        let mut files: Vec<_> = self.indexes.iter().collect();

        if self.filter_type != "all" {
            files.retain(|(filename, _)| self.get_file_type(filename) == self.filter_type);
        }

        if !self.search_filter.is_empty() {
            files.retain(|(filename, _)| {
                filename
                    .to_lowercase()
                    .contains(&self.search_filter.to_lowercase())
            });
        }

        match self.sort_by.as_str() {
            "name" => files.sort_by(|(a, _), (b, _)| a.cmp(b)),
            "size" => files.sort_by(|(_, a), (_, b)| a.length.cmp(&b.length)),
            "type" => {
                files.sort_by(|(a, _), (b, _)| self.get_file_type(a).cmp(self.get_file_type(b)))
            }
            _ => {}
        }

        if !self.sort_ascending {
            files.reverse();
        }

        files
    }

    fn get_archive_statistics(&self) -> String {
        let counts = self.count_files_by_type();
        let total_size: u64 = self.indexes.values().map(|e| e.length).sum();
        let modified_count = self.indexes.values().filter(|e| e.modified).count();
        let deleted_count = self.indexes.values().filter(|e| e.to_delete).count();

        format!(
            "📊 Archive Statistics\n\
            ═══════════════════════\n\n\
            📁 Total Files: {}\n\
            📦 Total Size: {}\n\
            ✏️ Modified: {}\n\
            🗑️ To Delete: {}\n\n\
            📊 By Type:\n\
            🖼️ Images: {}\n\
            🎬 Videos: {}\n\
            🎵 Audio: {}\n\
            📜 Scripts: {}\n\
            📄 Other: {}\n\n\
            🔧 Settings:\n\
            🗜️ Compression: Level {}\n\
            💾 Auto-backup: {}\n\
            📂 Backups: {}",
            self.indexes.len(),
            Self::format_bytes(total_size),
            modified_count,
            deleted_count,
            counts.get("images").unwrap_or(&0),
            counts.get("videos").unwrap_or(&0),
            counts.get("audio").unwrap_or(&0),
            counts.get("scripts").unwrap_or(&0),
            counts.get("other").unwrap_or(&0),
            self.compression_level,
            if self.auto_backup { "ON" } else { "OFF" },
            self.backup_history.len()
        )
    }

    fn batch_replace_from_folder(&mut self, folder_path: &str) -> anyhow::Result<usize> {
        let folder = Path::new(folder_path);
        let mut replaced_count = 0;

        for entry in std::fs::read_dir(folder)? {
            let entry = entry?;
            let file_path = entry.path();

            if file_path.is_file() {
                if let Some(filename) = file_path.file_name() {
                    let filename_str = filename.to_string_lossy().to_string();

                    if self.indexes.contains_key(&filename_str) {
                        match self.replace_file(&filename_str, &file_path.to_string_lossy()) {
                            Ok(()) => {
                                replaced_count += 1;
                                println!("🔄 Replaced: {}", filename_str);
                            }
                            Err(e) => {
                                println!("❌ Failed to replace {}: {}", filename_str, e);
                            }
                        }
                    }
                }
            }
        }

        self.status_message = format!("Batch replaced {} files", replaced_count);
        Ok(replaced_count)
    }
}

impl eframe::App for RpaEditor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(filename) = self.file_to_preview.take() {
            self.preview_file(&filename);
            self.selected_file = Some(filename);
        }

        if let Some(filename) = self.file_to_remove.take() {
            self.remove_file(&filename);
        }

        if let Some((filename, new_path)) = self.file_to_replace.take() {
            println!("{filename}, {new_path}");
            match self.replace_file(&filename, &new_path) {
                Ok(()) => {
                    if self.selected_file.as_ref() == Some(&filename) {
                        self.preview_file(&filename);
                    }
                }
                Err(e) => {
                    self.status_message = format!("Replace error: {}", e);
                }
            }
        }

        if let Some(folder_path) = self.batch_replace_to_execute.take() {
            match self.batch_replace_from_folder(&folder_path) {
                Ok(count) => {
                    self.status_message = format!("Batch replaced {} files", count);
                }
                Err(e) => {
                    self.status_message = format!("Batch replace error: {}", e);
                }
            }
        }

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open RPA").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("RPA files", &["rpa"])
                            .pick_file()
                        {
                            match self.load_rpa(&path.to_string_lossy()) {
                                Ok(()) => {}
                                Err(e) => {
                                    self.status_message = format!("Error loading: {}", e);
                                }
                            }
                        }
                        ui.close_menu();
                    }

                    if ui.button("Save").clicked() && self.archive_path.is_some() {
                        let path = self.archive_path.clone().unwrap();
                        match self.save_rpa(&path) {
                            Ok(()) => {}
                            Err(e) => {
                                self.status_message = format!("Save error: {}", e);
                            }
                        }
                        ui.close_menu();
                    }

                    if ui.button("Save As...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("RPA files", &["rpa"])
                            .save_file()
                        {
                            match self.save_rpa(&path.to_string_lossy()) {
                                Ok(()) => {}
                                Err(e) => {
                                    self.status_message = format!("Save error: {}", e);
                                }
                            }
                        }
                        ui.close_menu();
                    }
                });
                let filename = self.selected_file.clone();
                ui.menu_button("Tools", |ui| {
                    if ui.button("Add File...").clicked() {
                        self.show_add_dialog = true;
                        ui.close_menu();
                    }

                    ui.horizontal(|ui| {
                        if ui.button("🎯 Extract All Files").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                match self.dump_all_files(&folder) {
                                    Ok(count) => {
                                        self.status_message = format!(
                                            "Extracted {} files to organized folders",
                                            count
                                        )
                                    }
                                    Err(e) => self.status_message = format!("Extract Error: {}", e),
                                }
                                self.show_dump_dialog = false;
                            }
                        }
                        ui.label(format!("({} total files)", self.indexes.len()));
                    });

                    if ui.button("Replace...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Select replacement file")
                            .pick_file()
                        {
                            let file_path = path.to_string_lossy().to_string();
                            println!("🔍 Selected replacement file: {}", file_path);

                            if Path::new(&file_path).exists() {
                                self.file_to_replace = Some((filename.unwrap(), file_path));
                            } else {
                                self.status_message =
                                    format!("Selected file does not exist: {}", file_path);
                            }
                        }
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Archive Statistics").clicked() {
                        self.show_statistics_dialog = true;
                    }
                });
                if self.modified {
                    ui.colored_label(egui::Color32::YELLOW, "● Modified");
                }
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_message);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.indexes.is_empty() {
                        let counts = self.count_files_by_type();
                        ui.label(format!(
                            "🖼️{} 🎬{} 🎵{} 📜{}",
                            counts.get("images").unwrap_or(&0),
                            counts.get("videos").unwrap_or(&0),
                            counts.get("audio").unwrap_or(&0),
                            counts.get("scripts").unwrap_or(&0)
                        ));
                        ui.separator();

                        let visible_count = self.get_filtered_sorted_files().len();
                        if visible_count != self.indexes.len() {
                            ui.label(format!("{}/{} files", visible_count, self.indexes.len()));
                        } else {
                            ui.label(format!("{} files", self.indexes.len()));
                        }
                    }
                    if let Some(ref _path) = self.archive_path {
                        ui.separator();
                        ui.label(format!("RPA {:.1}", self.version));
                    }
                });
            });
        });

        egui::SidePanel::left("file_list")
            .resizable(true)
            .default_width(400.0)
            .width_range(300.0..=700.0)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.heading("📂 Files");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let visible_count = self.get_filtered_sorted_files().len();
                            if visible_count != self.indexes.len() {
                                ui.label(format!("{}/{}", visible_count, self.indexes.len()));
                            } else {
                                ui.label(format!("{}", self.indexes.len()));
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        for (filter, icon) in [
                            ("all", "📁"),
                            ("images", "🖼️"),
                            ("videos", "🎬"),
                            ("audio", "🎵"),
                            ("scripts", "📜"),
                        ] {
                            let is_selected = self.filter_type == filter;
                            if ui
                                .selectable_label(is_selected, format!("{} {}", icon, filter))
                                .clicked()
                            {
                                self.filter_type = filter.to_string();
                            }
                        }
                    });

                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("🔍");
                        ui.text_edit_singleline(&mut self.search_filter);
                        if ui.button("❌").clicked() {
                            self.search_filter.clear();
                        }
                    });

                    ui.separator();

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let files = self.get_filtered_sorted_files();

                            let mut file_to_select: Option<String> = None;
                            let mut file_to_preview: Option<String> = None;

                            for (filename, entry) in files {
                                let is_selected = Some(filename) == self.selected_file.as_ref();

                                let filename_clone = filename.clone();

                                ui.horizontal(|ui| {
                                    ui.set_min_height(25.0);

                                    ui.label(Self::get_file_icon(filename));

                                    let mut text = egui::RichText::new(filename);

                                    if entry.to_delete {
                                        text =
                                            text.strikethrough().color(egui::Color32::LIGHT_GRAY);
                                    } else if entry.modified {
                                        text = text.color(egui::Color32::LIGHT_YELLOW);
                                    } else {
                                        text = text.color(Self::get_file_type_color(filename));
                                    }

                                    if ui.selectable_label(is_selected, text).clicked() {
                                        file_to_select = Some(filename_clone.clone());
                                        file_to_preview = Some(filename_clone);
                                    }

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(Self::format_bytes(
                                                    entry.length,
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                        },
                                    );
                                });

                                ui.separator();
                            }

                            if let Some(selected) = file_to_select {
                                self.selected_file = Some(selected);
                            }
                            if let Some(preview) = file_to_preview {
                                self.file_to_preview = Some(preview);
                            }
                        });
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(ref selected) = self.selected_file.clone() {
                ui.horizontal(|ui| {
                    ui.heading(format!("{} {}", Self::get_file_icon(selected), selected));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ref _img) = self.preview_image {
                            ui.horizontal(|ui| {
                                ui.label("🔍");
                                if ui
                                    .add(
                                        egui::Slider::new(&mut self.image_zoom, 0.1..=3.0)
                                            .text("Zoom"),
                                    )
                                    .changed()
                                {}
                            });
                        }
                    });
                });

                ui.separator();

                ui.horizontal(|ui| {
                    let selected_clone = selected.clone();

                    if ui.button("📤 Extract").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name(&selected_clone)
                            .save_file()
                        {
                            if let Ok(data) = self.load_file_data(&selected_clone) {
                                if std::fs::write(&path, data).is_ok() {
                                    self.status_message = format!("Extracted {}", selected_clone);
                                }
                            }
                        }
                    }

                    if ui.button("🗑️ Remove").clicked() {
                        self.file_to_remove = Some(selected_clone.clone());
                    }

                    if ui.button("🔄 Replace").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.file_to_replace =
                                Some((path.to_string_lossy().to_string(), selected_clone.clone()));
                        }
                    }

                    if ui
                        .button(if self.is_playing { "Stop" } else { "Play" })
                        .clicked()
                    {
                        if self.is_playing {
                            self.audio_player.stop();
                            self.is_playing = false;
                        } else {
                            if let Ok(data) = self.load_file_data(&selected_clone) {
                                if selected_clone.ends_with(".ogg")
                                    || selected_clone.ends_with(".mp3")
                                    || selected_clone.ends_with(".wav")
                                    || selected_clone.ends_with(".flac")
                                {
                                    println!("Playing audio {}", selected_clone);
                                    self.audio_player.play_bytes(data);
                                    self.is_playing = true;
                                } else if selected_clone.ends_with(".mp4")
                                    || selected_clone.ends_with(".avi")
                                    || selected_clone.ends_with(".mov")
                                    || selected_clone.ends_with(".mkv")
                                    || selected_clone.ends_with(".webm")
                                {
                                    // play video
                                }
                            }
                        }
                    }

                    if ui.button("📁 Open Folder").clicked() {
                        if let Some(temp_dir) = std::env::temp_dir().parent() {
                            let extract_dir = temp_dir.join("rpa_editor_temp");
                            if create_dir_all(&extract_dir).is_ok() {
                                let file_path = extract_dir.join(&selected_clone);
                                if let Ok(data) = self.load_file_data(&selected_clone) {
                                    if let Some(parent) = file_path.parent() {
                                        let _ = create_dir_all(parent);
                                    }
                                    if std::fs::write(&file_path, data).is_ok() {
                                        #[cfg(target_os = "windows")]
                                        let _ = std::process::Command::new("explorer")
                                            .arg(&extract_dir)
                                            .spawn();

                                        #[cfg(target_os = "macos")]
                                        let _ = std::process::Command::new("open")
                                            .arg(&extract_dir)
                                            .spawn();

                                        #[cfg(target_os = "linux")]
                                        let _ = std::process::Command::new("xdg-open")
                                            .arg(&extract_dir)
                                            .spawn();

                                        self.status_message =
                                            format!("Opened folder for {}", selected_clone);
                                    }
                                }
                            }
                        }
                    }
                });

                ui.separator();

                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(ref img) = self.preview_image {
                            let texture =
                                ctx.load_texture("preview", img.clone(), Default::default());
                            let max_size = ui.available_size();
                            let img_size = egui::Vec2::new(img.width() as f32, img.height() as f32);

                            let base_scale = (max_size.x / img_size.x)
                                .min(max_size.y / img_size.y)
                                .min(1.0);
                            let display_size = img_size * base_scale * self.image_zoom;

                            ui.add(
                                egui::Image::new(&texture)
                                    .max_size(display_size)
                                    .maintain_aspect_ratio(true),
                            );

                            ui.separator();
                            ui.label(format!(
                                "Original: {}×{} | Display: {:.0}×{:.0} | Zoom: {:.1}%",
                                img.width(),
                                img.height(),
                                display_size.x,
                                display_size.y,
                                base_scale * self.image_zoom * 100.0
                            ));
                        } else if let Some(ref text) = self.preview_text {
                            let lines: Vec<&str> = text.lines().collect();
                            for line in lines {
                                if line.starts_with('#') {
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, line);
                                } else if line.contains("label")
                                    || line.contains("menu")
                                    || line.contains("scene")
                                {
                                    ui.colored_label(egui::Color32::LIGHT_BLUE, line);
                                } else if line.contains("Format:")
                                    || line.contains("Size:")
                                    || line.starts_with("🎬")
                                    || line.starts_with("🎵")
                                {
                                    ui.colored_label(egui::Color32::LIGHT_YELLOW, line);
                                } else if line.starts_with("✅") || line.starts_with("📊") {
                                    ui.colored_label(egui::Color32::CYAN, line);
                                } else {
                                    ui.label(line);
                                }
                            }
                        } else if let Some(ref data) = self.preview_data {
                            ui.horizontal(|ui| {
                                ui.label("📊 File size:");
                                ui.strong(Self::format_bytes(data.len() as u64));

                                ui.separator();

                                if ui.button("⬆️ Top").clicked() {
                                    self.hex_view_offset = 0;
                                }
                                if ui.button("⬇️ Next").clicked() {
                                    self.hex_view_offset = (self.hex_view_offset + 512)
                                        .min(data.len().saturating_sub(512));
                                }
                                if ui.button("⬆️ Prev").clicked() {
                                    self.hex_view_offset = self.hex_view_offset.saturating_sub(512);
                                }
                            });

                            ui.separator();

                            ui.heading("🔍 Hex Preview");

                            let start_offset = self.hex_view_offset;
                            let preview_bytes = std::cmp::min(512, data.len() - start_offset);

                            if preview_bytes > 0 {
                                let hex_dump = data[start_offset..start_offset + preview_bytes]
                                    .chunks(16)
                                    .enumerate()
                                    .map(|(i, chunk)| {
                                        let addr = start_offset + i * 16;
                                        let hex: String = chunk
                                            .iter()
                                            .map(|b| format!("{:02X}", b))
                                            .collect::<Vec<_>>()
                                            .join(" ");

                                        let ascii: String = chunk
                                            .iter()
                                            .map(|&b| {
                                                if b.is_ascii_graphic() || b == b' ' {
                                                    b as char
                                                } else {
                                                    '.'
                                                }
                                            })
                                            .collect();

                                        format!("{:08X}: {:<48} {}", addr, hex, ascii)
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");

                                ui.code(&hex_dump);

                                if start_offset + preview_bytes < data.len() {
                                    ui.label(format!(
                                        "... and {} more bytes",
                                        data.len() - start_offset - preview_bytes
                                    ));
                                }
                            }
                        }
                    });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("🎮 RPA Archive Editor - Enhanced");
                        ui.add_space(20.0);
                        ui.label("✨ NEW FEATURES:");
                        ui.label("• Enhanced WebP, WebM, OGG support");
                        ui.label("• Advanced .rpyc decompilation");
                        ui.label("• Batch replaces operations");
                        ui.label("• Advanced filtering and sorting");
                        ui.label("• Image zoom controls");
                        ui.label("• File statistics");
                        ui.add_space(10.0);
                        ui.label("Select a file from the list to preview");
                        ui.add_space(10.0);
                        if self.archive_path.is_none() {
                            ui.label("Open an RPA archive to get started");
                        }
                    });
                });
            }
        });

        if self.show_add_dialog {
            egui::Window::new("➕ Add File")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(450.0);

                    ui.horizontal(|ui| {
                        ui.label("📁 File:");
                        ui.text_edit_singleline(&mut self.add_file_path);
                        if ui.button("Browse...").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                self.add_file_path = path.to_string_lossy().to_string();
                                if self.add_file_name.is_empty() {
                                    self.add_file_name = path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string();
                                }
                            }
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("📝 Archive name:");
                        ui.text_edit_singleline(&mut self.add_file_name);
                    });

                    ui.separator();

                    ui.horizontal(|ui| {
                        if ui.button("✅ Add").clicked() {
                            if !self.add_file_path.is_empty() && !self.add_file_name.is_empty() {
                                let file_path = self.add_file_path.clone();
                                let file_name = self.add_file_name.clone();

                                if let Err(e) = self.add_file(&file_path, &file_name) {
                                    self.status_message = format!("Add Error: {}", e);
                                } else {
                                    self.show_add_dialog = false;
                                    self.add_file_path.clear();
                                    self.add_file_name.clear();
                                }
                            }
                        }

                        if ui.button("❌ Cancel").clicked() {
                            self.show_add_dialog = false;
                            self.add_file_path.clear();
                            self.add_file_name.clear();
                        }
                    });
                });
        }

        if self.show_batch_replace_dialog {
            egui::Window::new("📁 Batch Replace")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(500.0);

                    ui.label("Replace multiple files from a folder");
                    ui.label("Files with matching names will be replaced automatically");

                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("📂 Folder:");
                        ui.text_edit_singleline(&mut self.batch_replace_folder);
                        if ui.button("Browse...").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.batch_replace_folder = folder.to_string_lossy().to_string();
                            }
                        }
                    });

                    ui.separator();

                    ui.horizontal(|ui| {
                        if ui.button("🔄 Replace All").clicked() {
                            if !self.batch_replace_folder.is_empty() {
                                self.batch_replace_to_execute =
                                    Some(self.batch_replace_folder.clone());
                            }
                        }

                        if ui.button("❌ Cancel").clicked() {
                            self.show_batch_replace_dialog = false;
                            self.batch_replace_folder.clear();
                        }
                    });
                });
        }

        if self.show_statistics_dialog {
            egui::Window::new("📊 Archive Statistics")
                .collapsible(false)
                .resizable(true)
                .default_size([400.0, 500.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let stats = self.get_archive_statistics();
                        for line in stats.lines() {
                            if line.starts_with("📊") || line.starts_with("═") {
                                ui.heading(line);
                            } else if line.contains(':') {
                                ui.label(line);
                            } else {
                                ui.label(line);
                            }
                        }
                    });

                    ui.separator();
                    if ui.button("❌ Close").clicked() {
                        self.show_statistics_dialog = false;
                    }
                });
        }

        if self.show_backup_dialog {
            egui::Window::new("🔄 Backup History")
                .collapsible(false)
                .resizable(true)
                .default_size([500.0, 400.0])
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    if self.backup_history.is_empty() {
                        ui.label("No backups available");
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for backup in &self.backup_history {
                                ui.horizontal(|ui| {
                                    ui.label(format!("📄 {}", backup.filename));
                                    ui.label(format!(
                                        "({:.1} KB)",
                                        backup.data.len() as f32 / 1024.0
                                    ));
                                    ui.label(format!(
                                        "📅 {}",
                                        backup.timestamp.format("%Y-%m-%d %H:%M")
                                    ));

                                    if ui.button("📤 Restore").clicked() {
                                        let entry = RpaFileEntry {
                                            offset: 0,
                                            length: backup.data.len() as u64,
                                            prefix: Vec::new(),
                                            data: Some(backup.data.clone()),
                                            modified: true,
                                            to_delete: false,
                                        };
                                        self.indexes.insert(backup.filename.clone(), entry);
                                        self.modified = true;
                                        self.status_message =
                                            format!("Restored backup of {}", backup.filename);
                                    }
                                });
                                ui.separator();
                            }
                        });
                    }

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("🗑️ Clear All").clicked() {
                            self.backup_history.clear();
                            self.status_message = "Backup history cleared".to_string();
                        }
                        if ui.button("❌ Close").clicked() {
                            self.show_backup_dialog = false;
                        }
                    });
                });
        }

        if self.show_dump_dialog {
            egui::Window::new("📤 Bulk Extract")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(500.0);

                    ui.heading("Extract files by category");
                    ui.separator();

                    let counts = self.count_files_by_type();

                    ui.horizontal(|ui| {
                        if ui.button("🎯 Extract All Files").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                match self.dump_all_files(&folder) {
                                    Ok(count) => {
                                        self.status_message = format!(
                                            "Extracted {} files to organized folders",
                                            count
                                        )
                                    }
                                    Err(e) => self.status_message = format!("Extract Error: {}", e),
                                }
                                self.show_dump_dialog = false;
                            }
                        }
                        ui.label(format!("({} total files)", self.indexes.len()));
                    });

                    ui.separator();

                    for (file_type, icon) in [
                        ("images", "🖼️"),
                        ("videos", "🎬"),
                        ("audio", "🎵"),
                        ("scripts", "📜"),
                        ("other", "📄"),
                    ] {
                        let count = counts.get(file_type).unwrap_or(&0);
                        if *count > 0 {
                            ui.horizontal(|ui| {
                                if ui
                                    .button(format!(
                                        "{} Extract {}",
                                        icon,
                                        file_type.to_uppercase()
                                    ))
                                    .clicked()
                                {
                                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                        match self.dump_files_by_type(file_type, &folder) {
                                            Ok(extracted) => {
                                                self.status_message = format!(
                                                    "Extracted {} {} files",
                                                    extracted, file_type
                                                )
                                            }
                                            Err(e) => {
                                                self.status_message =
                                                    format!("Extract Error: {}", e)
                                            }
                                        }
                                        self.show_dump_dialog = false;
                                    }
                                }
                                ui.label(format!("({} files)", count));
                            });
                        }
                    }

                    ui.separator();

                    if ui.button("❌ Cancel").clicked() {
                        self.show_dump_dialog = false;
                    }
                });
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 1000.0])
            .with_title("🎮 RPA Archive Editor - Enhanced v2.0")
            .with_min_inner_size([900.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RPA Editor Enhanced",
        options,
        Box::new(|cc| Ok(Box::new(RpaEditor::new(cc)))),
    )
}

pub struct AudioPlayer {
    sink: Sink,
    _stream: OutputStream,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let (_stream, handle) =
            OutputStream::try_default().expect("Erreur lors de la création du périphérique audio");
        let sink = Sink::try_new(&handle).expect("Erreur lors de la création du Sink audio");
        Self { sink, _stream }
    }

    pub fn play_bytes(&self, data: Vec<u8>) {
        let cursor = Cursor::new(data);
        let source = Decoder::new(cursor);
        match source {
            Ok(e) => {
                self.sink.append(e);
                self.sink.play();
            }
            Err(e) => {
                eprintln!("Error playing audio: {}", e);
            }
        }
    }

    pub fn stop(&self) {
        self.sink.stop();
    }
}
