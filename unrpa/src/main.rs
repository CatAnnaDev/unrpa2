mod rpa;
mod toast;

use crate::rpa::{RpaEditor, RpaFileEntry};
use eframe::egui;
use egui_video::Player;
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::create_dir_all;
use std::io::Cursor;
use std::ops::Div;
use std::time::{Duration, Instant};

impl eframe::App for RpaEditor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();
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

        egui::TopBottomPanel::top("toasts_panel").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for toast in &self.toasts {
                    ui.label(
                        egui::RichText::new(&toast.message)
                            .background_color(egui::Color32::DARK_GREEN)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );
                }
            });
        });

        ctx.input(|i| {
            // Ctrl+O => Open RPA
            if i.key_pressed(egui::Key::O) && i.modifiers.ctrl {
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
            }
            // Ctrl+S => Save
            if i.key_pressed(egui::Key::S) && i.modifiers.ctrl && !i.modifiers.shift {
                if let Some(path) = self.archive_path.clone() {
                    match self.save_rpa(&path) {
                        Ok(()) => self.add_toast("Saved successfully"),
                        Err(e) => self.add_toast(format!("Save error: {}", e)),
                    }
                } else {
                    self.add_toast("No file to save");
                }
            }

            // Ctrl+Shift+S => Save As
            if i.key_pressed(egui::Key::S) && i.modifiers.ctrl && i.modifiers.shift {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("RPA files", &["rpa"])
                    .save_file()
                {
                    match self.save_rpa(&path.to_string_lossy()) {
                        Ok(()) => self.add_toast("Saved As successfully"),
                        Err(e) => self.add_toast(format!("Save error: {}", e)),
                    }
                }
            }

            // Ctrl+W => Close rpa
            if i.key_pressed(egui::Key::W) && i.modifiers.ctrl {
                if !self.modified {
                    if let Err(e) = self.unload_rpa() {
                        self.add_toast(format!("Error unloading: {}", e));
                    }
                } else {
                    self.show_close_confirm = true;
                }
            }
        });

        self.toasts.retain(|toast| !toast.is_expired());

        self.show_top_panel(ctx);

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_message);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.indexes.is_empty() {
                        let counts = self.count_files_by_type();
                        ui.label(format!(
                            "🖼️{} 🎬{} 🎵{} 📜{} {} {} {}",
                            counts.get("images").unwrap_or(&0),
                            counts.get("videos").unwrap_or(&0),
                            counts.get("audio").unwrap_or(&0),
                            counts.get("scripts").unwrap_or(&0),
                            counts.get("fonts").unwrap_or(&0),
                            counts.get("files").unwrap_or(&0),
                            counts.get("other").unwrap_or(&0),
                        ));
                        ui.separator();

                        let visible_count = self.get_filtered_sorted_files().len();
                        let total = self.indexes.len();
                        ui.label(if visible_count != total {
                            format!("{}/{} files", visible_count, total)
                        } else {
                            format!("{} files", total)
                        });
                    }
                    if self.archive_path.is_some() {
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
                            let total = self.indexes.len();
                            ui.label(if visible_count != total {
                                format!("{}/{}", visible_count, total)
                            } else {
                                format!("{}", total)
                            });
                        });
                    });

                    ui.horizontal(|ui| {
                        for (filter, icon) in [
                            ("all", "📁"),
                            ("images", "🖼️"),
                            ("videos", "🎬"),
                            ("audio", "🎵"),
                            ("scripts", "📜"),
                            ("fonts", "📜"),
                            ("files", "📜"),
                            ("other", "📜"),
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
                                        text = text.strikethrough().color(egui::Color32::RED);
                                    } else if entry.modified {
                                        text = text.color(egui::Color32::YELLOW);
                                    } else {
                                        text = text.color(Self::get_file_type_color(filename));
                                    }

                                    let label = ui.selectable_label(is_selected, text);

                                    if label.clicked() {
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
                            self.player = None;
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
                                    println!("Playing video {}", selected_clone);
                                    let byte_video = Player::from_bytes(ctx, &data).unwrap();
                                    if let None = byte_video.audio_streamer {
                                        self.player = Some(
                                            byte_video.with_audio(&mut self.audio_device).unwrap(),
                                        );
                                    } else {
                                        self.player = Some(byte_video);
                                    }
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
                
                if self.is_playing {
                    ui.group(|ui| {
                        ui.heading("🎧 Audio Controller");

                        if ui.button("⏸ Pause").clicked() {
                            self.audio_player.pause();
                        }

                        if ui.button("▶ Play").clicked() {
                            self.audio_player.resume();
                        }

                        if ui.button("⏹ Stop").clicked() {
                            self.audio_player.stop();
                        }

                        let mut volume = self.audio_player.get_volume();
                        if ui
                            .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("🔊 Volume"))
                            .changed()
                        {
                            self.audio_player.set_volume(volume);
                        }

                        if self.audio_player.is_finished() {
                            self.is_playing = false;
                        } else {
                            ui.label("🎵 En cours de lecture...");
                        }

                        if let Some(dur) = self.audio_player.total_duration() {
                            let pos = self.audio_player.playback_position();

                            let mut percent = pos.as_secs_f32() / dur.as_secs_f32();
                            percent = percent.clamp(0.0, 1.0);

                            ui.add_enabled_ui(false, |ui|{
                                ui.add(
                                    egui::Slider::new(&mut percent, 0.0..=1.0)
                                        .text(format!(
                                            "{:.0}/{:.0} sec",
                                            pos.as_secs_f32(),
                                            dur.as_secs_f32()
                                        )),
                                ); 
                            });
                        }
                    });
                }

                if let Some(player) = self.player.as_mut() {
                    player.ui(ui, player.size.div(2.5));
                }

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
                                    ui.colored_label(egui::Color32::from_rgb(0, 255, 255), line);
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
        centered: true,
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
    volume: f32,
    started_at: Option<Instant>,
    duration: Option<Duration>,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let (_stream, handle) =
            OutputStream::try_default().expect("Erreur lors de la création du périphérique audio");
        let sink = Sink::try_new(&handle).expect("Erreur lors de la création du Sink audio");
        sink.set_volume(1.0);
        Self {
            sink,
            _stream,
            volume: 1.0,
            started_at: None,
            duration: None,
        }
    }

    pub fn play_bytes(&mut self, data: Vec<u8>) {
        let cursor = Cursor::new(data.clone());
        match Decoder::new(cursor) {
            Ok(source) => {
                self.duration = source.total_duration();
                self.started_at = Some(Instant::now());
                self.sink.append(source);
                self.sink.play();
            }
            Err(e) => {
                eprintln!("Erreur de lecture audio: {}", e);
            }
        }
    }

    pub fn pause(&self) {
        self.sink.pause();
    }

    pub fn resume(&self) {
        self.sink.play();
    }

    pub fn stop(&self) {
        self.sink.stop();
    }

    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol;
        self.sink.set_volume(vol);
    }

    pub fn get_volume(&self) -> f32 {
        self.volume
    }

    pub fn is_finished(&self) -> bool {
        self.sink.empty()
    }

    pub fn playback_position(&self) -> Duration {
        if let Some(started) = self.started_at {
            if self.sink.is_paused() {
                Duration::ZERO
            } else {
                started.elapsed()
            }
        } else {
            Duration::ZERO
        }
    }

    pub fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}
