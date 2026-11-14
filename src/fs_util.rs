use std::fs::File;
use std::path::Path;

// fsyncs the directory that holds path. a new or renamed file only survives a crash
// once its directory entry is on disk too, fsyncing the file alone isn't enough
pub(crate) fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent: &Path = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(parent)?.sync_all()
}
