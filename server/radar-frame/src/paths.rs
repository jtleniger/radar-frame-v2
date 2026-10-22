//! File layout, rooted at the process working directory exactly as
//! `server/constants/paths.py` was.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Paths { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config").join("config.ini")
    }

    pub fn colors(&self) -> PathBuf {
        self.root.join("gdal-colors.txt")
    }

    pub fn log(&self) -> PathBuf {
        self.root.join("radar-frame.log")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn streets_osm(&self) -> PathBuf {
        self.data_dir().join("streets.osm")
    }

    pub fn streets_img(&self) -> PathBuf {
        self.data_dir().join("streets.png")
    }

    pub fn streets_bbox(&self) -> PathBuf {
        self.data_dir().join("streets.bbox")
    }

    pub fn streets_osm_bbox(&self) -> PathBuf {
        self.data_dir().join("streets.osm.bbox")
    }

    pub fn palette_img(&self) -> PathBuf {
        self.data_dir().join("palette.png")
    }

    pub fn radar_img(&self) -> PathBuf {
        self.data_dir().join("nexrad-latest.png")
    }

    pub fn radar_raw(&self) -> PathBuf {
        self.data_dir().join("nexrad-latest.ar2")
    }

    pub fn output_img(&self) -> PathBuf {
        self.data_dir().join("frame.png")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_live_under_the_data_directory() {
        let paths = Paths::new("/srv/radar");
        assert_eq!(paths.data_dir(), PathBuf::from("/srv/radar/data"));
        assert_eq!(
            paths.streets_img(),
            PathBuf::from("/srv/radar/data/streets.png")
        );
        assert_eq!(
            paths.output_img(),
            PathBuf::from("/srv/radar/data/frame.png")
        );
        assert_eq!(paths.colors(), PathBuf::from("/srv/radar/gdal-colors.txt"));
    }
}
