use std::collections::HashMap;
use std::fs::{create_dir_all, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde_pickle::{DeOptions, Value};
use crate::AudioPlayer;
use crate::toast::Toast;

#[derive(Debug, Clone)]
pub struct RpaFileEntry {
    pub offset: u64,
    pub length: u64,
    pub prefix: Vec<u8>,
    pub data: Option<Vec<u8>>,
    pub modified: bool,
    pub to_delete: bool,
}

#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub filename: String,
    pub data: Vec<u8>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct RpaEditor {
    pub version: f32,
    pub key: u32,
    pub indexes: HashMap<String, RpaFileEntry>,
    pub archive_path: Option<String>,
    pub modified: bool,
    pub selected_file: Option<String>,
    pub preview_data: Option<Vec<u8>>,
    pub preview_image: Option<egui::ColorImage>,
    pub preview_text: Option<String>,
    pub search_filter: String,
    pub show_add_dialog: bool,
    pub add_file_path: String,
    pub add_file_name: String,
    pub status_message: String,
    pub file_to_preview: Option<String>,
    pub file_to_remove: Option<String>,
    pub file_to_replace: Option<(String, String)>,
    pub batch_replace_to_execute: Option<String>,
    pub show_dump_dialog: bool,
    pub show_backup_dialog: bool,
    pub backup_history: Vec<BackupEntry>,
    pub show_batch_replace_dialog: bool,
    pub batch_replace_folder: String,
    pub show_statistics_dialog: bool,
    pub auto_backup: bool,
    pub compression_level: u32,
    pub filter_type: String,
    pub sort_by: String,
    pub sort_ascending: bool,
    pub image_zoom: f32,
    pub hex_view_offset: usize,
    pub audio_player: AudioPlayer,
    pub is_playing: bool,
    pub show_close_confirm: bool,
    pub(crate) toasts: Vec<Toast>,
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
            show_close_confirm: false,
            toasts: Vec::new(),
        }
    }
}

impl RpaEditor {
    pub(crate) fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    pub(crate) fn unload_rpa(&mut self) -> anyhow::Result<()> {
        self.version = 3.2;
        self.key = 0xDEADBEEF;
        self.indexes = HashMap::new();
        self.archive_path = None;
        self.modified = false;
        self.selected_file = None;
        self.preview_data = None;
        self.preview_image = None;
        self.preview_text = None;
        self.search_filter = String::new();
        self.show_add_dialog = false;
        self.add_file_path = String::new();
        self.add_file_name = String::new();
        self.status_message = "Ready".to_string();
        self.file_to_preview = None;
        self.file_to_remove = None;
        self.file_to_replace= None;
        self.batch_replace_to_execute= None;
        self.show_dump_dialog= false;

        self.show_backup_dialog= false;
        self.backup_history= Vec::new();
        self.show_batch_replace_dialog= false;
        self.batch_replace_folder= String::new();
        self.show_statistics_dialog= false;
        self.auto_backup = true;
        self.compression_level= 6;

        self.filter_type= "all".to_string();
        self.sort_by= "name".to_string();
        self.sort_ascending= true;

        self.image_zoom= 1.0;
        self.hex_view_offset= 0;
        self.audio_player= AudioPlayer::new();
        self.is_playing= false;
        Ok(())
    }

    pub(crate) fn load_rpa(&mut self, path: &str) -> anyhow::Result<()> {
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

    fn load_entries_data(&self, index: &mut HashMap<String, RpaFileEntry>, file: &mut File, ) -> anyhow::Result<()> {
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

    fn extract_indexes(&mut self, file: &mut File, ) -> anyhow::Result<HashMap<String, RpaFileEntry>> {
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

    fn find_entry_data_after_filename(&self, data: &[u8], start_pos: usize, ) -> Option<RpaFileEntry> {
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

    pub(crate) fn load_file_data(&self, filename: &str) -> anyhow::Result<Vec<u8>> {
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

    pub(crate) fn preview_file(&mut self, filename: &str) {
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
            } else if lower.ends_with(".rpy")
                || lower.ends_with(".py")
                || lower.ends_with(".json")
                || lower.ends_with(".txt")
                || lower.ends_with(".ini")
                || lower.ends_with(".xml")
                || lower.ends_with(".yaml")
                || lower.ends_with(".yml")
            {
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

        if data.len() > 20 {
            if data[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
                info.push_str("🎬 Format: WebM (VP8/VP9)\n");
                info.push_str("✅ Valid EBML header detected\n");
                info.push_str("📹 Container: Matroska-based\n");
            } else if &data[4..8] == b"ftyp" {
                info.push_str("🎬 Format: MP4 (H.264/H.265)\n");

                info.push_str("✅ Valid MP4 header detected\n");
                let brand = String::from_utf8_lossy(&data[8..12]);
                info.push_str(&format!("🏷️ Brand: {}\n", brand));
            } else if &data[0..4] == b"OggS" {
                info.push_str("🎵 Format: OGG Vorbis\n");

                info.push_str("✅ Valid OGG header detected\n");
                info.push_str("🎶 Codec: Vorbis audio\n");
            } else if &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
                info.push_str("🎵 Format: WAV (Uncompressed)\n");

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
            } else if &data[0..3] == b"ID3" {
                info.push_str("🎵 Format: MP3 (MPEG Audio)\n");

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

    pub(crate) fn replace_file(&mut self, filename: &str, new_file_path: &str) -> anyhow::Result<()> {
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

    pub(crate) fn add_file(&mut self, file_path: &str, archive_name: &str) -> anyhow::Result<()> {
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

    pub(crate) fn remove_file(&mut self, filename: &str) {
        if let Some(entry) = self.indexes.get_mut(filename) {
            entry.to_delete = true;
            self.modified = true;
            self.status_message = format!("Marked {} for deletion", filename);
        }
    }

    pub(crate) fn save_rpa(&self, archive_path: &str) -> anyhow::Result<()> {
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

    pub(crate) fn format_bytes(bytes: u64) -> String {
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

    pub(crate) fn get_file_icon(filename: &str) -> &'static str {
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

    pub(crate) fn get_file_type_color(filename: &str) -> egui::Color32 {
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
            || lower.ends_with(".gif")
            || lower.ends_with(".bmp")
        {
            "images"
        } else if lower.ends_with(".webm") || lower.ends_with(".mp4") || lower.ends_with(".avi") {
            "videos"
        } else if lower.ends_with(".ogg")
            || lower.ends_with(".wav")
            || lower.ends_with(".mp3")
            || lower.ends_with(".flac")
        {
            "audio"
        } else if lower.ends_with(".rpy") || lower.ends_with(".rpyc") || lower.ends_with(".py") {
            "scripts"
        } else if lower.ends_with(".ttf")
            || lower.ends_with(".otf")
            || lower.ends_with(".woff")
            || lower.ends_with(".woff2")
        {
            "fonts"
        } else if lower.ends_with(".json")
            || lower.ends_with(".txt")
            || lower.ends_with(".ini")
            || lower.ends_with(".xml")
            || lower.ends_with(".yaml")
            || lower.ends_with(".yml")
        {
            "files"
        } else {
            "other"
        }
    }

    pub(crate) fn count_files_by_type(&self) -> HashMap<&'static str, usize> {
        let mut counts = HashMap::new();
        for filename in self.indexes.keys() {
            let file_type = self.get_file_type(filename);
            *counts.entry(file_type).or_insert(0) += 1;
        }
        counts
    }

    pub(crate) fn dump_files_by_type(&self, file_type: &str, base_path: &Path) -> anyhow::Result<usize> {
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

    pub(crate) fn dump_all_files(&self, base_path: &Path) -> anyhow::Result<usize> {
        self.dump_files_by_type("all", base_path)
    }

    pub(crate) fn get_filtered_sorted_files(&self) -> Vec<(&String, &RpaFileEntry)> {
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

    pub(crate) fn get_archive_statistics(&self) -> String {
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

    pub(crate) fn batch_replace_from_folder(&mut self, folder_path: &str) -> anyhow::Result<usize> {
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

    pub(crate) fn show_file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.menu_button("File", |ui| {
            if ui.button("Open RPA").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("RPA files", &["rpa"])
                    .pick_file()
                {
                    if let Err(e) = self.load_rpa(&path.to_string_lossy()) {
                        self.add_toast(format!("Error loading: {}", e));
                    } else {
                        self.add_toast("RPA loaded successfully");
                    }
                }
                ui.close_menu();
            }

            if ui.button("Save").clicked() && self.archive_path.is_some() {
                let path = self.archive_path.clone().unwrap();
                match self.save_rpa(&path) {
                    Ok(_) => self.add_toast("Save Succes"),
                    Err(e) => self.add_toast(format!("Save error: {}", e)),
                    
                }
                ui.close_menu();
            }

            if ui.button("Save As...").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("RPA files", &["rpa"])
                    .save_file()
                {
                    match self.save_rpa(&path.to_string_lossy()) {
                        Ok(()) => self.add_toast(format!("Save Succes at {}", path.to_string_lossy())),
                        Err(e) => self.add_toast(format!("Save error: {}", e)),
                    }
                }
                ui.close_menu();
            }

            if ui.button("Close rpa").clicked() {
                if !self.modified {
                    if let Err(e) = self.unload_rpa() {
                        self.add_toast(format!("Error unloading: {}", e));
                    }
                } else {
                    self.show_close_confirm = true;
                }
                ui.close_menu();
            }
        });

        if self.show_close_confirm {
            egui::Window::new("Close Confirmation")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Are you sure you want to close without saving?");
                    ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() {
                            if let Err(e) = self.unload_rpa() {
                                self.add_toast(format!("Error unloading: {}", e));
                            }
                            self.show_close_confirm = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_close_confirm = false;
                        }
                    });
                });
        }
    }

    pub(crate) fn show_tools_menu(&mut self, ui: &mut egui::Ui) {
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
                                self.add_toast(format!(
                                    "Extracted {} files to organized folders",
                                    count
                                ))
                            }
                            Err(e) => self.add_toast(format!("Extract Error: {}", e)),
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
                        if let Some(filename) = self.selected_file.clone() {
                            self.file_to_replace = Some((filename, file_path));
                        } else {
                            self.add_toast("No file selected to replace".to_string());
                        }
                    } else {
                        self.add_toast(format!("Selected file does not exist: {}", file_path));
                    }
                }
                ui.close_menu();
            }
        });
    }

    pub(crate) fn show_view_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("View", |ui| {
            if ui.button("Add File").clicked() {
                self.show_add_dialog = true;
            }
            if ui.button("Batch Replace").clicked() {
                self.show_batch_replace_dialog = true;
            }
            if ui.button("Archive Statistics").clicked() {
                self.show_statistics_dialog = true;
            }
            if ui.button("Backup File").clicked() {
                self.show_backup_dialog = true;
            }
            if ui.button("Special Dump").clicked() {
                self.show_dump_dialog = true;
            }
        });
    }

    pub(crate) fn show_top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                self.show_file_menu(ui, ctx);
                self.show_tools_menu(ui);
                self.show_view_menu(ui);
                if self.modified {
                    ui.colored_label(egui::Color32::YELLOW, "● Modified");
                }
            });
        });
    }

    pub(crate) fn add_toast(&mut self, message: impl Into<String>) {
        self.toasts.push(Toast::new(message));
    }
}