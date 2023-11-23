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

fn main() {
    println!("import_converter");

    // read settings
    let settings = Config::builder()
        .add_source(config::File::with_name("icsettings"))
        .add_source(config::File::with_name("icsettings-user").required(false))
        .add_source(config::Environment::with_prefix("ICONV"))
        .build()
        .expect("unable to read settings (icsettings.toml needed, icsettings-user.toml optional)");

    // mask list
    let mut all_masks: Vec<MaskEntry> = Vec::new();
    for ext in settings.get_array("extensions.copy").expect("setting extensions.copy not found") {
        all_masks.push(MaskEntry { extension: ext.into_string().expect("setting extension.copy entry invalid"), reencode: false });
    }
    for ext in settings.get_array("extensions.reencode").expect("setting extensions.reencode not found") {
        all_masks.push(MaskEntry { extension: ext.into_string().expect("setting extension.reencode entry invalid"), reencode: true });
    }
    for entry in &all_masks {
        println!("{} --> reencode: {:?}", entry.extension, entry.reencode);
    }

    // file list
    let mut all_files: Vec<FileEntry> = Vec::new();

    // discover files
    println!("collecting files...");
    for mask in &all_masks {
        // build glob pattern for each extension in an OS independent way by using PathBuf
        let mut source_dir = PathBuf::from(settings.get_string("directories.source").expect("setting directories.source not found"));
        source_dir.push("**");
        source_dir.push(&mask.extension);
        for entry in glob(source_dir.as_path().to_str().expect("error building glob pattern")).expect("Failed to read glob pattern") {
            match entry {
                Ok(path) => {
                    // add entry
                    all_files.push(build_file_entry(&settings, &path, &mask));
                },
                Err(e) => println!("{:?}", e),
            }
        }
    }

    // display files and planned actions
    let len_sum: u64 = all_files.iter().map(|f| f.len).sum();
    let len_sum_mb = len_sum as f32 / (1024.0 * 1024.0);
    println!("found {} files (total of {len_sum_mb:.2} MB):", all_files.len());
    for entry in &all_files {
        let len_mb = entry.len as f32 / (1024.0 * 1024.0);
        println!("{} ({:.2} MB) - taken {:?} ({}) --> {} ({})",
            entry.source_path.display().to_string(), len_mb, entry.creation_date, entry.creation_date_source.to_string(),
            entry.destination_path.display().to_string(), if entry.reencode {"reencode"} else {"copy"});
    }

    // dry run ends here
    if settings.get_bool("options.dry-run").expect("setting options.dry-run not found") {
        println!("Dry run configured --> exiting.");
        return;
    }

    // process the files as planned
    println!("processing {} files...", all_files.len());
    for entry in &mut all_files {
        // make sure destination directory exists (otherwise handbrake will fail)
        fs::create_dir_all(entry.destination_path.parent().expect("entry destination path broken")).expect("create_dir_all failed for entry");

        if entry.reencode {
            // reencode
            let handbrake_cli = settings.get_string("encoding.handbrakeCLI").expect("setting encoding.handbrakeCLI not found");
            let handbrake_options = settings.get_string("encoding.options").expect("setting encoding.options not found");
            // https://handbrake.fr/docs/en/latest/cli/cli-options.html
            let handbrake_call = format!("\"{}\" -i \"{}\" -o \"{}\" {}",
                handbrake_cli,
                entry.source_path.display(), entry.destination_path.display(),
                handbrake_options);
            println!("{handbrake_call:}");

            // call handbrake
            let mut hndbrk = std::process::Command::new(handbrake_cli);
            hndbrk.args(["-i", &entry.source_path.display().to_string()])
                .args(["-o", &entry.destination_path.display().to_string()])
                .args(handbrake_options.split_ascii_whitespace());

            let mut child = hndbrk.spawn().expect("failed to execute HandbrakeCLI");

            let status = child.wait();
            match status {
                Ok(status) => {
                    entry.success = status.success();
                    println!("Handbrake execution finished, status --> {:?}", entry.success);
                    if entry.success {
                        // file size
                        let metadata = fs::metadata(&entry.destination_path);
                        entry.len_after_action = metadata.expect("unable to read file metadata").len();
                    }
                },
                Err(e) => {
                    entry.success = false;
                    println!("Handbrake execution error --> {:?}", e);
                },
            }
            
        } else {
            // copy
            match fs::copy(&entry.source_path, &entry.destination_path) {
                Ok(bytes_copied) => {
                    entry.len_after_action = bytes_copied;
                    entry.success = true;
                    let len_mb = bytes_copied as f32 / (1024.0 * 1024.0);
                    println!("{:?} copied --> {:?} ({:.2} MB)", entry.source_path, entry.destination_path, len_mb);
                },
                Err(e) => {
                    entry.success = false;
                    println!("Copy error --> {:?}", e);
                },
            }
        }
    }

    // todo: refactor creation_date function (--> impl for FileEntry?)
    // todo: if configured, delete source files after successful operation
    
    // show summary (e.g. total size before/after, files copied/reencoded, etc)
    let len_sum_after: u64 = all_files.iter().map(|f| f.len_after_action).sum();
    let len_sum_after_mb = len_sum_after as f32 / (1024.0 * 1024.0);
    let success_count: u32 = all_files.iter().map(|f| if f.success {1} else {0}).sum();
    let reencode_success_count: u32 = all_files.iter().map(|f| if f.reencode && f.success {1} else {0}).sum();
    let copy_success_count: u32 = all_files.iter().map(|f| if !f.reencode && f.success {1} else {0}).sum();
    let reencode_fail_count: u32 = all_files.iter().map(|f| if f.reencode && !f.success {1} else {0}).sum();
    let copy_fail_count: u32 = all_files.iter().map(|f| if !f.reencode && !f.success {1} else {0}).sum();
    println!("");
    println!("__Summary__");
    println!("Total number of files: {}, of these successfuly processed: {success_count}", all_files.len());
    println!("Copy:     success count: {copy_success_count}, fail count: {copy_fail_count}");
    println!("Reencode: success count: {reencode_success_count}, fail count: {reencode_fail_count}");
    println!("Filesize: before: {len_sum_mb:.2} MB, after: {len_sum_after_mb:.2} MB");
    
}

fn build_file_entry(settings: &Config, path: &PathBuf, mask: &MaskEntry) -> FileEntry {
    // get file creation date
    let (dto, dto_source) = get_file_creation(&path).expect("failed to get file creation date");
    // file size
    let metadata = fs::metadata(&path);
    let file_len = metadata.expect("failed to read file metadata").len();
    // build FileEntry
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
    
    // change file extension
    if result.reencode {
        result.destination_path.set_extension(settings.get_string("encoding.new_file_extension").expect("setting encoding.new_file_extension not found"));
    }

    return result;
}

fn build_destination_path(settings: &Config, dto: &NaiveDateTime, path: &PathBuf) -> PathBuf {
    let destination = settings.get_string("directories.destination").expect("setting directories.destination not found");
    let destination_format = settings.get_string("organisation.path_format").expect("setting organisation.path_format not found");
    let new_subpath = dto.format(&destination_format);
    let prefix_format = settings.get_string("organisation.filename_prefix").expect("setting organisation.filename_prefix not found");
    let prefix = dto.format(&prefix_format);
    let file_name = path.file_name().expect("filename not part of destination path").to_str().expect("filename to string failed");
    // build new path in an OS independent way
    let mut new_path = PathBuf::from(destination);
    new_path.push(new_subpath.to_string());
    let new_file_name = prefix.to_string() + file_name;
    new_path.set_file_name(new_file_name);

    return new_path;
}

// get file creation DateTime via EXIF, MP4 Metadata, file system properties or the local time
fn get_file_creation(path: &std::path::Path) -> Result<(chrono::NaiveDateTime, DateSource), Box<dyn error::Error>>  {
    // try EXIF first
    {
        let file = std::fs::File::open(path)?;
        let mut bufreader = std::io::BufReader::new(&file);
        let exifreader = exif::Reader::new();
        let exif = exifreader.read_from_container(&mut bufreader);

        /*
        for f in exif.fields() {
            println!("{} {} {}",
                    f.tag, f.ifd_num, f.display_value().with_unit(&exif));
        }
        */

        match exif {
            Ok(exif) => {
                match exif.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
                    Some(dto) => {
                        let ds: &str = &dto.display_value().to_string();
                        let dt = chrono::NaiveDateTime::parse_from_str(ds, "%Y-%m-%d %H:%M:%S")?;
                        return Ok((dt, DateSource::Exif));
                    },
                    None => eprintln!("creation tag is missing"),
                }
            }
            Err(_) => {
                //println!("EXIF Reader: {:?}", e);
            }
        }
    }

    // try to read metadata from MOV file format
    {
        let f = File::open(path)?;
        let mp4 = mp4::read_mp4(f);

        match mp4 {
            Ok(mp4) => {
                let mut creation_time = mp4.moov.mvhd.creation_time;
                // convert from MP4 epoch (1904-01-01) to Unix epoch (1970-01-01)
                if creation_time >= 2082844800 {
                    creation_time -= 2082844800
                }
                // https://stackoverflow.com/questions/72884445/chrono-datetime-from-u64-unix-timestamp-in-rust
                if let chrono::LocalResult::Single(dtl) = chrono::Local.timestamp_opt(creation_time as i64, 0){
                    let dtn = dtl.naive_local();
                    return Ok((dtn, DateSource::Mp4Metadata));
                }                
            },
            Err(_) => {
                //println!("MP4 Reader: {:?}", e);
            },
        }
        
    }

    // fallback to filesystem information
    let metadata = fs::metadata(path)?;
    // ToDo: use modified and created and use the older date    
    if let Ok(time) = metadata.modified() {
        let dtl: chrono::DateTime<Local> = time.clone().into();
        let dtn = dtl.naive_local();
        return Ok((dtn, DateSource::FileSystem));
    } else {
        println!("filesystem metadata 'modified' not supported");
    }

    // fallback to current time
    Ok((Local::now().naive_local(), DateSource::LocalTime))
}
