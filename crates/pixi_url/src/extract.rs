//! Helpers to extract archives.
use std::{ffi::OsStr, io::BufRead, path::Path};

use fs_err as fs;
use fs_err::File;
use indicatif::{ProgressBar, ProgressFinish};

use crate::{error::ExtractError, progress::ProgressHandler};

/// Handle compression formats internally.
enum TarCompression<'a> {
    PlainTar(Box<dyn BufRead + 'a>),
    Gzip(flate2::read::GzDecoder<Box<dyn BufRead + 'a>>),
    Bzip2(bzip2::read::BzDecoder<Box<dyn BufRead + 'a>>),
    Xz2(xz2::read::XzDecoder<Box<dyn BufRead + 'a>>),
    Zstd(zstd::stream::read::Decoder<'a, std::io::BufReader<Box<dyn BufRead + 'a>>>),
}

/// Checks whether file has known tarball extension.
pub fn is_tarball(file_name: &str) -> bool {
    [
        // Gzip
        ".tar.gz",
        ".tgz",
        ".taz",
        // Bzip2
        ".tar.bz2",
        ".tbz",
        ".tbz2",
        ".tz2",
        // Xz2
        ".tar.lzma",
        ".tlz",
        ".tar.xz",
        ".txz",
        // Zstd
        ".tar.zst",
        ".tzst",
        // PlainTar
        ".tar",
    ]
    .iter()
    .any(|ext| file_name.ends_with(ext))
}

/// Checks whether file has a known archive extension (including zip).
pub fn is_archive(file_name: &str) -> bool {
    is_tarball(file_name) || file_name.ends_with(".zip") || file_name.ends_with(".7z")
}

fn ext_to_compression<'a>(
    ext: Option<&OsStr>,
    file: Box<dyn BufRead + 'a>,
) -> Result<TarCompression<'a>, ExtractError> {
    match ext
        .and_then(OsStr::to_str)
        .and_then(|s| s.rsplit_once('.'))
        .map(|(_, s)| s)
    {
        Some("gz" | "tgz" | "taz") => Ok(TarCompression::Gzip(flate2::read::GzDecoder::new(file))),
        Some("bz2" | "tbz" | "tbz2" | "tz2") => {
            Ok(TarCompression::Bzip2(bzip2::read::BzDecoder::new(file)))
        }
        Some("lzma" | "tlz" | "xz" | "txz") => {
            Ok(TarCompression::Xz2(xz2::read::XzDecoder::new(file)))
        }
        Some("zst" | "tzst") => Ok(TarCompression::Zstd(
            zstd::stream::read::Decoder::new(file)
                .map_err(|err| ExtractError::TarExtractionError(err.to_string()))?,
        )),
        Some("Z" | "taZ") => Err(ExtractError::UnsupportedCompression("compress")),
        Some("lz") => Err(ExtractError::UnsupportedCompression("lzip")),
        Some("lzo") => Err(ExtractError::UnsupportedCompression("lzo")),
        Some(_) | None => Ok(TarCompression::PlainTar(file)),
    }
}

impl std::io::Read for TarCompression<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            TarCompression::PlainTar(reader) => reader.read(buf),
            TarCompression::Gzip(reader) => reader.read(buf),
            TarCompression::Bzip2(reader) => reader.read(buf),
            TarCompression::Xz2(reader) => reader.read(buf),
            TarCompression::Zstd(reader) => reader.read(buf),
        }
    }
}

/// Moves the directory content from src to dest after stripping root dir, if present.
fn move_extracted_dir(src: &Path, dest: &Path) -> Result<(), ExtractError> {
    let mut entries = fs::read_dir(src)?;
    let src_dir = match entries.next().transpose()? {
        // ensure if only single directory in entries(root dir)
        Some(dir) if entries.next().is_none() && dir.file_type()?.is_dir() => {
            src.join(dir.file_name())
        }
        _ => src.to_path_buf(),
    };

    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        let destination = dest.join(entry.file_name());
        fs::rename(entry.path(), destination)?;
    }

    Ok(())
}

fn bytes_progress_bar(handler: &dyn ProgressHandler, len: u64, prefix: &str) -> ProgressBar {
    let bar = ProgressBar::new(len).with_style(handler.default_bytes_style());
    bar.set_prefix(prefix.to_string());
    handler.add_progress_bar(bar)
}

/// Extracts a tar archive to the specified target directory.
pub fn extract_tar(
    archive: impl AsRef<Path>,
    target_directory: impl AsRef<Path>,
    handler: &dyn ProgressHandler,
) -> Result<(), ExtractError> {
    let archive = archive.as_ref();
    let target_directory = target_directory.as_ref();

    fs::create_dir_all(target_directory)?;

    let len = archive.metadata().map(|m| m.len()).unwrap_or(1);
    let progress_bar = bytes_progress_bar(handler, len, "Extracting tar");

    let file = File::open(archive)?;
    let buf_reader = std::io::BufReader::with_capacity(1024 * 1024, file);
    let wrapped = progress_bar.wrap_read(buf_reader);

    let mut archive =
        tar::Archive::new(ext_to_compression(archive.file_name(), Box::new(wrapped))?);

    let tmp_extraction_dir = tempfile::Builder::new().tempdir_in(target_directory)?;
    archive
        .unpack(&tmp_extraction_dir)
        .map_err(|e| ExtractError::TarExtractionError(e.to_string()))?;

    move_extracted_dir(tmp_extraction_dir.path(), target_directory)?;
    progress_bar.finish_with_message("Extracted...");

    Ok(())
}

/// Extracts a zip archive to the specified target directory.
/// Currently this doesn't support bzip2 and zstd.
///
/// `.zip` files archived with compression other than deflate would fail.
pub fn extract_zip(
    archive: impl AsRef<Path>,
    target_directory: impl AsRef<Path>,
    handler: &dyn ProgressHandler,
) -> Result<(), ExtractError> {
    let archive = archive.as_ref();
    let target_directory = target_directory.as_ref();
    fs::create_dir_all(target_directory)?;

    let len = archive.metadata().map(|m| m.len()).unwrap_or(1);
    let progress_bar = handler.add_progress_bar(
        ProgressBar::new(len)
            .with_finish(ProgressFinish::AndLeave)
            .with_prefix("Extracting zip")
            .with_style(handler.default_bytes_style()),
    );

    let file = File::open(archive)?;
    let buf_reader = std::io::BufReader::with_capacity(1024 * 1024, file);
    let wrapped = progress_bar.wrap_read(buf_reader);
    let mut archive =
        zip::ZipArchive::new(wrapped).map_err(|e| ExtractError::InvalidZip(e.to_string()))?;

    let tmp_extraction_dir = tempfile::Builder::new().tempdir_in(target_directory)?;
    archive
        .extract(&tmp_extraction_dir)
        .map_err(|e| ExtractError::ZipExtractionError(e.to_string()))?;

    move_extracted_dir(tmp_extraction_dir.path(), target_directory)?;
    progress_bar.finish_with_message("Extracted...");

    Ok(())
}

/// Extracts a 7z archive to the specified target directory.
pub fn extract_7z(
    archive: impl AsRef<Path>,
    target_directory: impl AsRef<Path>,
    handler: &dyn ProgressHandler,
) -> Result<(), ExtractError> {
    let archive = archive.as_ref();
    let target_directory = target_directory.as_ref();
    fs::create_dir_all(target_directory)?;

    let len = archive.metadata().map(|m| m.len()).unwrap_or(1);
    let progress_bar = bytes_progress_bar(handler, len, "Extracting 7z");

    let file = File::open(archive)?;
    let buf_reader = std::io::BufReader::with_capacity(1024 * 1024, file);
    let wrapped = progress_bar.wrap_read(buf_reader);

    let tmp_extraction_dir = tempfile::Builder::new().tempdir_in(target_directory)?;
    sevenz_rust2::decompress(wrapped, &tmp_extraction_dir)
        .map_err(|e| ExtractError::SevenZipExtractionError(e.to_string()))?;

    move_extracted_dir(tmp_extraction_dir.path(), target_directory)?;

    progress_bar.finish_with_message("Extracted...");
    Ok(())
}

#[cfg(test)]
mod test {
    use fs_err::{self as fs, File};
    use std::io::Write;

    use super::extract_zip;
    use crate::{error::ExtractError, progress::NoProgressHandler};

    #[test]
    fn test_extract_zip() {
        // zip contains text.txt with "Hello, World" text
        const HELLO_WORLD_ZIP_FILE: &[u8] = &[
            80, 75, 3, 4, 10, 0, 0, 0, 0, 0, 244, 123, 36, 88, 144, 58, 246, 64, 13, 0, 0, 0, 13,
            0, 0, 0, 8, 0, 28, 0, 116, 101, 120, 116, 46, 116, 120, 116, 85, 84, 9, 0, 3, 4, 130,
            150, 101, 6, 130, 150, 101, 117, 120, 11, 0, 1, 4, 245, 1, 0, 0, 4, 20, 0, 0, 0, 72,
            101, 108, 108, 111, 44, 32, 87, 111, 114, 108, 100, 10, 80, 75, 1, 2, 30, 3, 10, 0, 0,
            0, 0, 0, 244, 123, 36, 88, 144, 58, 246, 64, 13, 0, 0, 0, 13, 0, 0, 0, 8, 0, 24, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 164, 129, 0, 0, 0, 0, 116, 101, 120, 116, 46, 116, 120, 116, 85,
            84, 5, 0, 3, 4, 130, 150, 101, 117, 120, 11, 0, 1, 4, 245, 1, 0, 0, 4, 20, 0, 0, 0, 80,
            75, 5, 6, 0, 0, 0, 0, 1, 0, 1, 0, 78, 0, 0, 0, 79, 0, 0, 0, 0, 0,
        ];

        let tempdir = tempfile::tempdir().unwrap();
        let file_path = tempdir.path().join("test.zip");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(HELLO_WORLD_ZIP_FILE).unwrap();

        let handler = NoProgressHandler::default();
        let res = extract_zip(file_path, tempdir.path(), &handler);
        assert!(res.is_ok(), "zip extraction failed: {res:?}");
        assert!(tempdir.path().join("text.txt").exists());
        assert!(
            fs::read_to_string(tempdir.path().join("text.txt"))
                .unwrap()
                .contains("Hello, World")
        );
    }

    #[test]
    fn test_extract_fail() {
        let handler = NoProgressHandler::default();
        let tempdir = tempfile::tempdir().unwrap();
        let result = extract_zip("", tempdir.path(), &handler);
        assert!(
            matches!(result, Err(ExtractError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn test_extract_fail_invalid_zip() {
        let handler = NoProgressHandler::default();
        let tempdir = tempfile::tempdir().unwrap();
        let file = tempdir.path().join("test.zip");
        File::create(&file).unwrap();
        let res = extract_zip(file, tempdir.path(), &handler);
        assert!(matches!(res, Err(ExtractError::InvalidZip(_))));
    }
}
