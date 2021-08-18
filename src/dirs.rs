use std::path::{Path, PathBuf};

use crate::metadata::{NAME, ORGANIZATION, QUALIFIER};
use directories::ProjectDirs;

fn project_directories() -> ProjectDirs {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, NAME).unwrap()
}

fn cache_dir<P>(sub_directory: P) -> PathBuf
where
    P: AsRef<Path>,
{
    let project_directories = project_directories();
    let cache_directory = project_directories.cache_dir();
    let mut directory: PathBuf = cache_directory.into();
    directory.push(sub_directory);
    directory
}

pub fn layer_dir() -> PathBuf {
    cache_dir("layers")
}

pub fn blob_dir() -> PathBuf {
    cache_dir("blobs")
}

pub fn containers() -> PathBuf {
    cache_dir("containers")
}
