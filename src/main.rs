/*
 * SPDX-FileCopyrightText: 2023 Maik Fox <maik.fox@gmail.com>
 *
 * SPDX-License-Identifier: EUPL-1.2
 */

use config::Config;
use exif::{Tag, In};
use glob::glob;
use chrono::prelude::*;
use std::fmt;
use std::fs::File;
use std::path::PathBuf;
use std::error;
use std::fs;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    // Options
    pub dry_run: bool,
    pub delete_source_files: bool,
    
    // Directories
    pub source_dir: String,
    pub destination_dir: String,
    
    // Extensions
    pub copy_extensions: Vec<String>,
    pub reencode_extensions: Vec<String>,
    
    // Organization
    pub path_format: String,
    pub filename_prefix: String,
    
    // Encoding
    pub handbrake_cli: String,
    pub handbrake_options: String,
    pub new_file_extension: String,
}

impl AppSettings {
    pub fn from_config() -> Result<Self, Box<dyn error::Error>> {
        let settings = Config::builder()
            .add_source(config::File::with_name("icsettings"))
            .add_source(config::File::with_name("icsettings-user").required(false))
            .add_source(config::Environment::with_prefix("ICONV"))
            .build()?;

        let copy_extensions: Vec<String> = settings.get_array("extensions.copy")?
            .iter()
            .map(|v| v.clone().into_string().unwrap_or_default())
            .collect();
            
        let reencode_extensions: Vec<String> = settings.get_array("extensions.reencode")?
            .iter()
            .map(|v| v.clone().into_string().unwrap_or_default())
            .collect();

        Ok(AppSettings {
            dry_run: settings.get_bool("options.dry-run")?,
            delete_source_files: settings.get_bool("options.delete_source_files")?,
            source_dir: settings.get_string("directories.source")?,
            destination_dir: settings.get_string("directories.destination")?,
            copy_extensions,
            reencode_extensions,
            path_format: settings.get_string("organisation.path_format")?,
            filename_prefix: settings.get_string("organisation.filename_prefix")?,
            handbrake_cli: settings.get_string("encoding.handbrakeCLI")?,
            handbrake_options: settings.get_string("encoding.options")?,
            new_file_extension: settings.get_string("encoding.new_file_extension")?,
        })
    }
}

#[derive(Debug, Clone)]
pub enum ProcessingMessage {
    Progress(String),
    FileProcessed(String),
    Summary(String),
    Error(String),
    Finished,
}

pub struct ImportConverterApp {
    settings: AppSettings,
    processing: bool,
    progress_receiver: Option<mpsc::Receiver<ProcessingMessage>>,
    log_messages: Vec<String>,
    copy_extensions_text: String,
    reencode_extensions_text: String,
}

impl Default for ImportConverterApp {
    fn default() -> Self {
        let settings = AppSettings::from_config().unwrap_or_default();
        let copy_extensions_text = settings.copy_extensions.join(", ");
        let reencode_extensions_text = settings.reencode_extensions.join(", ");
        
        Self {
            settings,
            processing: false,
            progress_receiver: None,
            log_messages: Vec::new(),
            copy_extensions_text,
            reencode_extensions_text,
        }
    }
}

impl eframe::App for ImportConverterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check for progress messages
        if let Some(receiver) = &self.progress_receiver {
            while let Ok(message) = receiver.try_recv() {
                match message {
                    ProcessingMessage::Progress(msg) => {
                        self.log_messages.push(msg);
                    }
                    ProcessingMessage::FileProcessed(msg) => {
                        self.log_messages.push(msg);
                    }
                    ProcessingMessage::Summary(msg) => {
                        self.log_messages.push(msg);
                    }
                    ProcessingMessage::Error(msg) => {
                        self.log_messages.push(format!("ERROR: {}", msg));
                    }
                    ProcessingMessage::Finished => {
                        self.processing = false;
                        //self.progress_receiver = None;
                        self.log_messages.push("Processing completed!".to_string());
                    }
                }
                ctx.request_repaint();
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Import Converter");
            
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Options Section
                ui.collapsing("Options", |ui| {
                    ui.checkbox(&mut self.settings.dry_run, "Dry run (don't actually process files)");
                    ui.checkbox(&mut self.settings.delete_source_files, "Delete source files after processing");
                });

                ui.separator();

                // Directories Section
                ui.collapsing("Directories", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Source Directory:");
                        ui.text_edit_singleline(&mut self.settings.source_dir);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Destination Directory:");
                        ui.text_edit_singleline(&mut self.settings.destination_dir);
                    });
                });

                ui.separator();

                // Extensions Section
                ui.collapsing("File Extensions", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Copy Extensions:");
                        ui.text_edit_singleline(&mut self.copy_extensions_text);
                    });
                    ui.small("Comma-separated list (e.g., *.JPG, *.JPEG)");
                    
                    ui.horizontal(|ui| {
                        ui.label("Reencode Extensions:");
                        ui.text_edit_singleline(&mut self.reencode_extensions_text);
                    });
                    ui.small("Comma-separated list (e.g., *.MOV)");
                });

                ui.separator();

                // Organization Section
                ui.collapsing("Organization", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Path Format:");
                        ui.text_edit_singleline(&mut self.settings.path_format);
                    });
                    ui.small("Date format for folder structure (e.g., %Y\\%m\\%Y-%m-%d)");
                    
                    ui.horizontal(|ui| {
                        ui.label("Filename Prefix:");
                        ui.text_edit_singleline(&mut self.settings.filename_prefix);
                    });
                    ui.small("Prefix for filenames (e.g., %Y-%m-%d_)");
                });

                ui.separator();

                // Encoding Section
                ui.collapsing("Encoding Settings", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("HandBrakeCLI Path:");
                        ui.text_edit_singleline(&mut self.settings.handbrake_cli);
                    });
                    
                    ui.horizontal(|ui| {
                        ui.label("HandBrake Options:");
                        ui.text_edit_singleline(&mut self.settings.handbrake_options);
                    });
                    ui.small("CLI options for encoding (e.g., -e nvenc_h264 -q 22 -B 160)");
                    
                    ui.horizontal(|ui| {
                        ui.label("New File Extension:");
                        ui.text_edit_singleline(&mut self.settings.new_file_extension);
                    });
                });

                ui.separator();

                // Process Button
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.processing, egui::Button::new("Start Processing")).clicked() {
                        self.start_processing();
                    }
                    
                    if self.processing {
                        ui.spinner();
                        ui.label("Processing...");
                    }
                });

                ui.separator();

                // Log Output
                ui.collapsing("Log Output", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for message in &self.log_messages {
                                ui.label(message);
                            }
                        });
                    
                    if ui.button("Clear Log").clicked() {
                        self.log_messages.clear();
                    }
                });
            });
        });
    }
}

impl ImportConverterApp {
    fn start_processing(&mut self) {
        // Parse extensions from text
        self.settings.copy_extensions = self.copy_extensions_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
            
        self.settings.reencode_extensions = self.reencode_extensions_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let (tx, rx) = mpsc::channel();
        self.progress_receiver = Some(rx);
        self.processing = true;
        self.log_messages.clear();

        let settings = self.settings.clone();
        thread::spawn(move || {
            process_files(settings, tx);
        });
    }
}

fn process_files(settings: AppSettings, tx: mpsc::Sender<ProcessingMessage>) {
    let _ = tx.send(ProcessingMessage::Progress("Starting file processing...".to_string()));

    // Build mask list
    let mut all_masks: Vec<MaskEntry> = Vec::new();
    for ext in &settings.copy_extensions {
        all_masks.push(MaskEntry { extension: ext.clone(), reencode: false });
    }
    for ext in &settings.reencode_extensions {
        all_masks.push(MaskEntry { extension: ext.clone(), reencode: true });
    }

    let _ = tx.send(ProcessingMessage::Progress(format!("Configured {} file masks", all_masks.len())));

    // File discovery
    let mut all_files: Vec<FileEntry> = Vec::new();
    let _ = tx.send(ProcessingMessage::Progress("Discovering files...".to_string()));

    for mask in &all_masks {
        let mut source_dir = PathBuf::from(&settings.source_dir);
        source_dir.push("**");
        source_dir.push(&mask.extension);
        
        if let Ok(entries) = glob(source_dir.as_path().to_str().unwrap_or_default()) {
            for entry in entries {
                match entry {
                    Ok(path) => {
                        if let Ok(file_entry) = build_file_entry(&settings, &path, &mask) {
                            all_files.push(file_entry);
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ProcessingMessage::Error(format!("Glob error: {:?}", e)));
                    }
                }
            }
        }
    }

    let len_sum: u64 = all_files.iter().map(|f| f.len).sum();
    let len_sum_mb = len_sum as f32 / (1024.0 * 1024.0);
    let _ = tx.send(ProcessingMessage::Progress(format!("Found {} files (total of {:.2} MB)", all_files.len(), len_sum_mb)));

    // Display planned actions
    for entry in &all_files {
        let len_mb = entry.len as f32 / (1024.0 * 1024.0);
        let _ = tx.send(ProcessingMessage::Progress(format!(
            "{} ({:.2} MB) - taken {:?} ({}) --> {} ({})",
            entry.source_path.display(),
            len_mb,
            entry.creation_date,
            entry.creation_date_source,
            entry.destination_path.display(),
            if entry.reencode { "reencode" } else { "copy" }
        )));
    }

    if settings.dry_run {
        let _ = tx.send(ProcessingMessage::Progress("Dry run configured - no files will be processed.".to_string()));
        let _ = tx.send(ProcessingMessage::Finished);
        return;
    }

    // Process files
    let _ = tx.send(ProcessingMessage::Progress(format!("Processing {} files...", all_files.len())));
    let mut processed_files = all_files;
    
    for entry in &mut processed_files {
        // Create destination directory
        if let Some(parent) = entry.destination_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                let _ = tx.send(ProcessingMessage::Error(format!("Failed to create directory: {:?}", e)));
                continue;
            }
        }

        if entry.reencode {
            // Reencode with HandBrake
            let _ = tx.send(ProcessingMessage::Progress(format!("Reencoding: {}", entry.source_path.display())));
            
            let mut hndbrk = std::process::Command::new(&settings.handbrake_cli);
            hndbrk.args(["-i", &entry.source_path.display().to_string()])
                .args(["-o", &entry.destination_path.display().to_string()])
                .args(settings.handbrake_options.split_ascii_whitespace());

            match hndbrk.spawn() {
                Ok(mut child) => {
                    match child.wait() {
                        Ok(status) => {
                            entry.success = status.success();
                            if entry.success {
                                if let Ok(metadata) = fs::metadata(&entry.destination_path) {
                                    entry.len_after_action = metadata.len();
                                }
                                let _ = tx.send(ProcessingMessage::FileProcessed(format!("Reencoded: {}", entry.destination_path.display())));
                            } else {
                                let _ = tx.send(ProcessingMessage::Error(format!("HandBrake failed for: {}", entry.source_path.display())));
                            }
                        }
                        Err(e) => {
                            entry.success = false;
                            let _ = tx.send(ProcessingMessage::Error(format!("HandBrake execution error: {:?}", e)));
                        }
                    }
                }
                Err(e) => {
                    entry.success = false;
                    let _ = tx.send(ProcessingMessage::Error(format!("Failed to start HandBrake: {:?}", e)));
                }
            }
        } else {
            // Copy file
            let _ = tx.send(ProcessingMessage::Progress(format!("Copying: {}", entry.source_path.display())));
            
            match fs::copy(&entry.source_path, &entry.destination_path) {
                Ok(bytes_copied) => {
                    entry.len_after_action = bytes_copied;
                    entry.success = true;
                    let len_mb = bytes_copied as f32 / (1024.0 * 1024.0);
                    let _ = tx.send(ProcessingMessage::FileProcessed(format!("Copied: {} ({:.2} MB)", entry.destination_path.display(), len_mb)));
                }
                Err(e) => {
                    entry.success = false;
                    let _ = tx.send(ProcessingMessage::Error(format!("Copy error: {:?}", e)));
                }
            }
        }
    }

    // Generate summary
    let len_sum_after: u64 = processed_files.iter().map(|f| f.len_after_action).sum();
    let len_sum_after_mb = len_sum_after as f32 / (1024.0 * 1024.0);
    let success_count: u32 = processed_files.iter().map(|f| if f.success { 1 } else { 0 }).sum();
    let reencode_success_count: u32 = processed_files.iter().map(|f| if f.reencode && f.success { 1 } else { 0 }).sum();
    let copy_success_count: u32 = processed_files.iter().map(|f| if !f.reencode && f.success { 1 } else { 0 }).sum();
    let reencode_fail_count: u32 = processed_files.iter().map(|f| if f.reencode && !f.success { 1 } else { 0 }).sum();
    let copy_fail_count: u32 = processed_files.iter().map(|f| if !f.reencode && !f.success { 1 } else { 0 }).sum();

    let _ = tx.send(ProcessingMessage::Summary("=== Summary ===".to_string()));
    let _ = tx.send(ProcessingMessage::Summary(format!("Total files: {}, Successfully processed: {}", processed_files.len(), success_count)));
    let _ = tx.send(ProcessingMessage::Summary(format!("Copy: {} success, {} failed", copy_success_count, copy_fail_count)));
    let _ = tx.send(ProcessingMessage::Summary(format!("Reencode: {} success, {} failed", reencode_success_count, reencode_fail_count)));
    let _ = tx.send(ProcessingMessage::Summary(format!("File size: before {:.2} MB, after {:.2} MB", len_sum_mb, len_sum_after_mb)));

    let _ = tx.send(ProcessingMessage::Finished);
}

// Rest of the existing structs and functions
pub struct MaskEntry {
    extension: String,
    reencode: bool,
}

enum DateSource {
    Exif,
    Mp4Metadata,
    FileSystem,
    LocalTime,
}

impl fmt::Display for DateSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DateSource::Exif => write!(f, "EXIF"),
            DateSource::Mp4Metadata => write!(f, "MP4 Metadata"),
            DateSource::FileSystem => write!(f, "File Attribute"),
            DateSource::LocalTime => write!(f, "Local Time"),
        }
    }
}

pub struct FileEntry {
    source_path: PathBuf,
    destination_path: PathBuf,
    creation_date: chrono::NaiveDateTime,
    creation_date_source: DateSource,
    len: u64,
    reencode: bool,
    success: bool,
    len_after_action: u64,
}

fn build_file_entry(settings: &AppSettings, path: &PathBuf, mask: &MaskEntry) -> Result<FileEntry, Box<dyn error::Error>> {
    let (dto, dto_source) = get_file_creation(&path)?;
    let metadata = fs::metadata(&path)?;
    let file_len = metadata.len();
    
    let mut result = FileEntry {
        source_path: PathBuf::from(&path.display().to_string()),
        destination_path: build_destination_path(&settings, &dto, &path),
        creation_date: dto,
        creation_date_source: dto_source,
        len: file_len,
        reencode: mask.reencode,
        success: false,
        len_after_action: 0,
    };
    
    if result.reencode {
        result.destination_path.set_extension(&settings.new_file_extension);
    }

    Ok(result)
}

fn build_destination_path(settings: &AppSettings, dto: &NaiveDateTime, path: &PathBuf) -> PathBuf {
    let new_subpath = dto.format(&settings.path_format);
    let prefix = dto.format(&settings.filename_prefix);
    let file_name = path.file_name().unwrap().to_str().unwrap();
    
    let mut new_path = PathBuf::from(&settings.destination_dir);
    new_path.push(new_subpath.to_string());
    let new_file_name = prefix.to_string() + file_name;
    new_path.push(new_file_name);

    new_path
}

fn get_file_creation(path: &std::path::Path) -> Result<(chrono::NaiveDateTime, DateSource), Box<dyn error::Error>> {
    // Try EXIF first
    if let Ok(file) = std::fs::File::open(path) {
        let mut bufreader = std::io::BufReader::new(&file);
        let exifreader = exif::Reader::new();
        if let Ok(exif) = exifreader.read_from_container(&mut bufreader) {
            if let Some(dto) = exif.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
                let ds: &str = &dto.display_value().to_string();
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ds, "%Y-%m-%d %H:%M:%S") {
                    return Ok((dt, DateSource::Exif));
                }
            }
        }
    }

    // Try MP4 metadata
    if let Ok(f) = File::open(path) {
        if let Ok(mp4) = mp4::read_mp4(f) {
            let mut creation_time = mp4.moov.mvhd.creation_time;
            if creation_time >= 2082844800 {
                creation_time -= 2082844800;
            }
            if let chrono::LocalResult::Single(dtl) = chrono::Local.timestamp_opt(creation_time as i64, 0) {
                let dtn = dtl.naive_local();
                return Ok((dtn, DateSource::Mp4Metadata));
            }
        }
    }

    // Fallback to filesystem
    let metadata = fs::metadata(path)?;
    if let Ok(time) = metadata.modified() {
        let dtl: chrono::DateTime<Local> = time.into();
        let dtn = dtl.naive_local();
        return Ok((dtn, DateSource::FileSystem));
    }

    // Fallback to current time
    Ok((Local::now().naive_local(), DateSource::LocalTime))
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Import Converter",
        options,
        Box::new(|_cc| Ok(Box::new(ImportConverterApp::default()))),
    )
}