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

    pub fn log(&self) -> PathBuf {
        self.root.join("radar-frame.log")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// Photos for [`crate::state::Mode::Photo`]. Fixed at the working
    /// directory root, alongside `data/`, and never created by the service.
    pub fn photos_dir(&self) -> PathBuf {
        self.root.join("photos")
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

    /// Content fingerprint of [`Self::streets_osm`], written after parsing.
    pub fn streets_osm_hash(&self) -> PathBuf {
        self.data_dir().join("streets.osm.hash")
    }

    /// Staging path for a newly rasterized mask; only renamed over
    /// [`Self::streets_img`] after it has been validated.
    pub fn streets_img_part(&self) -> PathBuf {
        self.data_dir().join("streets.png.part")
    }

    pub fn palette_img(&self) -> PathBuf {
        self.data_dir().join("palette.png")
    }

    pub fn radar_img(&self) -> PathBuf {
        self.data_dir().join("nexrad-latest.png")
    }

    /// Staging path for a downloaded volume; only renamed to [`Self::radar_raw`]
    /// after the volume has been parsed and rendered.
    pub fn radar_part(&self) -> PathBuf {
        self.data_dir().join("nexrad-latest.ar2.part")
    }

    /// Staging path for the radar frame rendered from [`Self::radar_part`];
    /// committed together with the volume.
    pub fn radar_img_part(&self) -> PathBuf {
        self.data_dir().join("nexrad-latest.png.part")
    }

    pub fn radar_raw(&self) -> PathBuf {
        self.data_dir().join("nexrad-latest.ar2")
    }

    pub fn output_img(&self) -> PathBuf {
        self.data_dir().join("frame.png")
    }

    /// Persisted schedule, mode selection, and radar provenance.
    pub fn state_file(&self) -> PathBuf {
        self.data_dir().join("state.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_live_under_the_data_directory() {
        let paths = Paths::new("/srv/radar");
        assert_eq!(paths.data_dir(), PathBuf::from("/srv/radar/data"));
        assert_eq!(paths.photos_dir(), PathBuf::from("/srv/radar/photos"));
        assert_eq!(
            paths.streets_img(),
            PathBuf::from("/srv/radar/data/streets.png")
        );
        assert_eq!(
            paths.output_img(),
            PathBuf::from("/srv/radar/data/frame.png")
        );
        assert_eq!(
            paths.state_file(),
            PathBuf::from("/srv/radar/data/state.json")
        );
    }
}
