use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use glob::glob;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Help,
    Trash,
    List,
    Empty,
    RestoreOne,
    RestoreAll,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    force: bool,
    restore_name: Option<String>,
    targets: Vec<String>,
}

#[derive(Debug)]
struct TrashEntry {
    trash_name: String,
    info_path: PathBuf,
    trashed_path: PathBuf,
    original_path: PathBuf,
    deletion_date: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(env::args().skip(1))?;

    match args.mode {
        Mode::Help => {
            println!("{}", usage(""));
            Ok(())
        }
        Mode::List => {
            let entries = load_entries().map_err(io_error)?;
            if entries.is_empty() {
                println!("Trash is empty.");
                return Ok(());
            }

            println!("{:30}  {:19}  Original path", "Name in trash", "Deleted");
            println!("{}", "-".repeat(90));
            for e in entries {
                let deleted = e.deletion_date.unwrap_or_else(|| "(unknown)".to_string());
                println!(
                    "{:30}  {:19}  {}",
                    e.trash_name,
                    deleted,
                    e.original_path.display()
                );
            }
            Ok(())
        }
        Mode::Empty => {
            empty_trash().map_err(io_error)?;
            println!("Trash emptied.");
            Ok(())
        }
        Mode::RestoreAll => {
            let entries = load_entries().map_err(io_error)?;
            if entries.is_empty() {
                println!("Trash is empty.");
                return Ok(());
            }

            let mut restored = 0usize;
            for e in entries {
                match restore_entry(&e) {
                    Ok(path) => {
                        restored += 1;
                        println!("Restored: {}", path.display());
                    }
                    Err(err) => eprintln!("warning: could not restore '{}': {err}", e.trash_name),
                }
            }
            println!("Restored {restored} item(s).");
            Ok(())
        }
        Mode::RestoreOne => {
            let name = args
                .restore_name
                .ok_or_else(|| "missing filename for -restore".to_string())?;
            let entries = load_entries().map_err(io_error)?;

            let matches: Vec<_> = entries
                .into_iter()
                .filter(|e| {
                    e.trash_name == name
                        || e
                            .original_path
                            .file_name()
                            .and_then(OsStr::to_str)
                            .map(|n| n == name)
                            .unwrap_or(false)
                })
                .collect();

            if matches.is_empty() {
                return Err(format!("no trashed item found for '{name}'"));
            }

            for e in matches {
                let restored_to = restore_entry(&e).map_err(io_error)?;
                println!("Restored: {}", restored_to.display());
            }
            Ok(())
        }
        Mode::Trash => {
            if args.targets.is_empty() {
                return Err(usage("No files/folders provided."));
            }

            let targets = expand_targets(&args.targets, args.force)?;
            if targets.is_empty() {
                if args.force {
                    return Ok(());
                }
                return Err("no matching files/folders were found".to_string());
            }

            for path in targets {
                trash::delete(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                println!("Trashed: {}", path.display());
            }
            Ok(())
        }
    }
}

fn parse_args<I>(args: I) -> Result<Args, String>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut mode = Mode::Trash;
    let mut force = false;
    let mut restore_name = None;
    let mut targets = Vec::new();

    let argv: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut i = 0usize;
    while i < argv.len() {
        match argv[i].as_str() {
            "-h" | "--help" => mode = Mode::Help,
            "-f" | "--force" => {
                force = true;
            }
            "-list" => mode = set_mode(mode, Mode::List)?,
            "-empty" => mode = set_mode(mode, Mode::Empty)?,
            "-restore-all" => mode = set_mode(mode, Mode::RestoreAll)?,
            "-restore" => {
                mode = set_mode(mode, Mode::RestoreOne)?;
                i += 1;
                let val = argv
                    .get(i)
                    .ok_or_else(|| usage("-restore requires a filename"))?
                    .clone();
                restore_name = Some(val);
            }
            other => targets.push(other.to_string()),
        }
        i += 1;
    }

    Ok(Args {
        mode,
        force,
        restore_name,
        targets,
    })
}

fn set_mode(current: Mode, new_mode: Mode) -> Result<Mode, String> {
    if current != Mode::Trash && current != new_mode {
        return Err(usage("only one operation flag can be used at a time"));
    }
    Ok(new_mode)
}

fn usage(prefix: &str) -> String {
    let header = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}\n\n")
    };

    format!(
        "{header}Usage:\n  trash <file|folder|glob>...\n  trash -list\n  trash -empty\n  trash -restore <filename>\n  trash -restore-all\n\nOptions:\n  -f, --force     ignore missing paths/patterns\n  -h, --help      show this help"
    )
}

fn expand_targets(inputs: &[String], force: bool) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();

    for item in inputs {
        if has_glob_chars(item) {
            let mut matched = false;
            for entry in glob(item).map_err(|e| e.to_string())? {
                matched = true;
                let path = entry.map_err(|e| e.to_string())?;
                out.push(path);
            }
            if !matched && !force {
                return Err(format!("pattern matched nothing: {item}"));
            }
        } else {
            let path = PathBuf::from(item);
            if path.exists() {
                out.push(path);
            } else if !force {
                return Err(format!("path not found: {item}"));
            }
        }
    }

    Ok(out)
}

fn has_glob_chars(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

fn xdg_data_home() -> Result<PathBuf, io::Error> {
    if let Ok(val) = env::var("XDG_DATA_HOME") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }

    let home = env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))?;

    Ok(home.join(".local/share"))
}

fn trash_base_dir() -> Result<PathBuf, io::Error> {
    Ok(xdg_data_home()?.join("Trash"))
}

fn ensure_trash_dirs() -> Result<(PathBuf, PathBuf), io::Error> {
    let base = trash_base_dir()?;
    let files = base.join("files");
    let info = base.join("info");
    fs::create_dir_all(&files)?;
    fs::create_dir_all(&info)?;
    Ok((files, info))
}

fn load_entries() -> Result<Vec<TrashEntry>, io::Error> {
    let (files_dir, info_dir) = ensure_trash_dirs()?;
    let mut entries = Vec::new();

    for file in fs::read_dir(&info_dir)? {
        let file = file?;
        let path = file.path();
        if path.extension().and_then(OsStr::to_str) != Some("trashinfo") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        let stem = stem.to_string();

        let text = fs::read_to_string(&path)?;
        let mut original_path: Option<PathBuf> = None;
        let mut deletion_date: Option<String> = None;

        for line in text.lines() {
            if let Some(encoded) = line.strip_prefix("Path=") {
                let decoded = urlencoding::decode(encoded)
                    .map(|v| v.into_owned())
                    .unwrap_or_else(|_| encoded.to_string());
                original_path = Some(PathBuf::from(decoded));
            } else if let Some(date) = line.strip_prefix("DeletionDate=") {
                deletion_date = Some(date.to_string());
            }
        }

        if let Some(orig) = original_path {
            entries.push(TrashEntry {
                trash_name: stem.clone(),
                info_path: path,
                trashed_path: files_dir.join(&stem),
                original_path: orig,
                deletion_date,
            });
        }
    }

    entries.sort_by(|a, b| a.trash_name.cmp(&b.trash_name));
    Ok(entries)
}

fn restore_entry(entry: &TrashEntry) -> Result<PathBuf, io::Error> {
    if !entry.trashed_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("trashed data missing: {}", entry.trashed_path.display()),
        ));
    }

    let mut dest = entry.original_path.clone();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    if dest.exists() {
        dest = next_available_path(&dest);
    }

    fs::rename(&entry.trashed_path, &dest)?;
    if entry.info_path.exists() {
        fs::remove_file(&entry.info_path)?;
    }

    Ok(dest)
}

fn next_available_path(path: &Path) -> PathBuf {
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("restored");

    let (stem, ext) = split_name(file_name);

    for i in 1.. {
        let candidate = if ext.is_empty() {
            format!("{stem}.restored-{i}")
        } else {
            format!("{stem}.restored-{i}.{ext}")
        };

        let candidate_path = parent.join(candidate);
        if !candidate_path.exists() {
            return candidate_path;
        }
    }

    unreachable!();
}

fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((left, right)) if !left.is_empty() && !right.is_empty() => (left, right),
        _ => (name, ""),
    }
}

fn empty_trash() -> Result<(), io::Error> {
    let (files_dir, info_dir) = ensure_trash_dirs()?;

    clear_dir(&files_dir)?;
    clear_dir(&info_dir)?;
    Ok(())
}

fn clear_dir(dir: &Path) -> Result<(), io::Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;

        if meta.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }

    Ok(())
}

fn io_error(err: io::Error) -> String {
    err.to_string()
}
