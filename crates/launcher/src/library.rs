//! the music on a card: every file that plays, found once, as paths from the
//! card's root. the folders shown are read back out of those paths.

use crate::decode::EXTENSIONS;
use std::path::Path;

/// between the folders of a track's path, whatever the host puts there
pub const SEPARATOR: char = '/';
/// folders inside folders followed this far, so a loop of links ends
const MAX_DEPTH: usize = 8;

/// a line of a folder's listing
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry<'a> {
    /// a folder inside the one listed, by its name
    Folder(&'a str),
    /// a track, by its place among all of them
    Track(usize),
}

fn plays(name: &str) -> bool {
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension).to_ascii_lowercase();
    EXTENSIONS.contains(&extension.as_str())
}

/// add the tracks in a directory and the ones inside it. what cannot be
/// read is left out, and so are hidden files, which on a card from a mac
/// are not music whatever their names end in
fn collect(directory: &Path, prefix: &str, depth: usize, tracks: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)?.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        match (name.starts_with('.'), is_dir) {
            (true, _) => {}
            (_, true) if depth > 0 => {
                drop(collect(&entry.path(), &format!("{prefix}{name}{SEPARATOR}"), depth - 1, tracks))
            }
            (_, false) if plays(&name) => tracks.push(format!("{prefix}{name}")),
            _ => {}
        }
    }
    Ok(())
}

/// every track under a directory, in the order they are listed and played:
/// by path, whatever the case
pub fn scan(root: &Path) -> std::io::Result<Vec<String>> {
    let mut tracks = Vec::new();
    collect(root, "", MAX_DEPTH, &mut tracks)?;
    tracks.sort_by_cached_key(|track| (track.to_lowercase(), track.clone()));
    Ok(tracks)
}

/// what a folder holds: its folders, then its tracks. the root is ""
pub fn entries<'a>(tracks: &'a [String], folder: &str) -> Vec<Entry<'a>> {
    let (mut folders, mut files) = (Vec::new(), Vec::new());
    for (index, track) in tracks.iter().enumerate() {
        let inside = match folder {
            "" => Some(track.as_str()),
            _ => track.strip_prefix(folder).and_then(|rest| rest.strip_prefix(SEPARATOR)),
        };
        match inside.map(|rest| rest.split_once(SEPARATOR)) {
            // sorted, a folder's tracks follow each other
            Some(Some((name, _))) if folders.last() != Some(&Entry::Folder(name)) => folders.push(Entry::Folder(name)),
            Some(None) => files.push(Entry::Track(index)),
            _ => {}
        }
    }
    folders.extend(files);
    folders
}

/// the folder a folder is in, and its own name there
pub fn split(folder: &str) -> (&str, &str) {
    folder.rsplit_once(SEPARATOR).unwrap_or(("", folder))
}

/// what a track is listed as: its file's name without the ending
pub fn title(track: &str) -> &str {
    let name = split(track).1;
    name.rsplit_once('.').map_or(name, |(title, _)| title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks() -> Vec<String> {
        ["a.mp3", "Album/01 one.flac", "Album/02 two.flac", "Album/live/x.wav", "b.wav"].map(String::from).to_vec()
    }

    #[test]
    fn lists_a_folders_folders_before_its_tracks() {
        let tracks = tracks();
        assert_eq!(entries(&tracks, ""), [Entry::Folder("Album"), Entry::Track(0), Entry::Track(4)]);
        assert_eq!(entries(&tracks, "Album"), [Entry::Folder("live"), Entry::Track(1), Entry::Track(2)]);
        assert_eq!(entries(&tracks, "Album/live"), [Entry::Track(3)]);
        assert_eq!(entries(&tracks, "Alb"), []);
    }

    #[test]
    fn names_tracks_and_folders() {
        assert_eq!(title("Album/01 one.flac"), "01 one");
        assert_eq!(title("no ending"), "no ending");
        assert_eq!(split("Album/live"), ("Album", "live"));
        assert_eq!(split("Album"), ("", "Album"));
    }

    #[test]
    fn finds_the_music_under_a_directory_in_order() {
        let root = std::env::temp_dir().join(format!("behead-library-{}", std::process::id()));
        for file in ["b.MP3", "A/2.flac", "A/1.wav", "A/cover.jpg", ".hidden/x.mp3", "._b.mp3", "a/deep/er/z.mp3"] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let found = scan(&root);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(found.unwrap(), ["A/1.wav", "A/2.flac", "a/deep/er/z.mp3", "b.MP3"]);
        assert!(scan(&root).is_err(), "a directory that is not there has no music");
    }
}
