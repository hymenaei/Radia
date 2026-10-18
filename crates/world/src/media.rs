use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    mem::size_of,
};

use crate::MAX_SURFACE_FACES;

/// Maximum UTF-8 byte length of an ObjectMedia URL field.
pub const MAX_MEDIA_URL_BYTES: usize = 1_024;
/// Maximum number of domains in one media whitelist.
pub const MAX_MEDIA_WHITELIST_ENTRIES: usize = 64;
/// Maximum combined UTF-8 byte length of one media whitelist.
pub const MAX_MEDIA_WHITELIST_BYTES: usize = 1_024;
/// Maximum pixel dimension for an ObjectMedia entry.
pub const MAX_MEDIA_DIMENSION: u16 = 2_048;
const MEDIA_PERMISSION_MASK: u8 = 0x07;

/// Media controls shown for a face.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MediaControls {
    /// Standard media controls.
    #[default]
    Standard,
    /// Compact media controls.
    Mini,
}

/// One Second Life media entry attached to a surface face.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaEntry {
    pub alt_image_enable: bool,
    pub controls: MediaControls,
    pub current_url: String,
    pub home_url: String,
    pub auto_loop: bool,
    pub auto_play: bool,
    pub auto_scale: bool,
    pub auto_zoom: bool,
    pub first_click_interact: bool,
    pub width_pixels: u16,
    pub height_pixels: u16,
    pub whitelist_enable: bool,
    pub whitelist: Vec<String>,
    pub perms_interact: u8,
    pub perms_control: u8,
}

impl Default for MediaEntry {
    fn default() -> Self {
        Self {
            alt_image_enable: false,
            controls: MediaControls::Standard,
            current_url: String::new(),
            home_url: String::new(),
            auto_loop: false,
            auto_play: false,
            auto_scale: false,
            auto_zoom: false,
            first_click_interact: false,
            width_pixels: 0,
            height_pixels: 0,
            whitelist_enable: false,
            whitelist: Vec::new(),
            perms_interact: MEDIA_PERMISSION_MASK,
            perms_control: MEDIA_PERMISSION_MASK,
        }
    }
}

/// A versioned, per-face ObjectMedia response patch.
///
/// Array positions map to TextureEntry faces 0–44. `None` clears that face, while a missing
/// suffix leaves later faces unchanged when the patch is applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMediaUpdate {
    version: u32,
    faces: Vec<Option<MediaEntry>>,
}

impl ObjectMediaUpdate {
    /// Builds a response patch after checking its face and entry bounds.
    pub fn new(version: u32, faces: Vec<Option<MediaEntry>>) -> Result<Self, ObjectMediaError> {
        let update = Self { version, faces };
        update.validate()?;
        Ok(update)
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn faces(&self) -> &[Option<MediaEntry>] {
        &self.faces
    }

    pub(crate) fn validate(&self) -> Result<(), ObjectMediaError> {
        if self.faces.len() > MAX_SURFACE_FACES {
            return Err(ObjectMediaError::TooManyFaces);
        }
        for entry in self.faces.iter().flatten() {
            entry.validate()?;
        }
        Ok(())
    }
}

/// The latest accepted ObjectMedia version and its sparse face values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObjectMedia {
    version: u32,
    faces: Vec<(u8, MediaEntry)>,
}

impl ObjectMedia {
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Returns media data for faces 0–44.
    ///
    /// Out-of-range indices and faces without media return `None`.
    pub fn face(&self, index: u8) -> Option<&MediaEntry> {
        if usize::from(index) >= MAX_SURFACE_FACES {
            return None;
        }
        self.faces.binary_search_by_key(&index, |(face_index, _)| *face_index).ok().map(|slot| &self.faces[slot].1)
    }

    /// Returns sorted face entries. Cleared and absent faces are omitted.
    pub fn faces(&self) -> impl Iterator<Item = (u8, &MediaEntry)> {
        self.faces.iter().map(|(index, entry)| (*index, entry))
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.faces.iter().fold(
            size_of::<Self>().saturating_add(self.faces.capacity().saturating_mul(size_of::<(u8, MediaEntry)>())),
            |total, (_, entry)| {
                total
                    .saturating_add(entry.current_url.capacity())
                    .saturating_add(entry.home_url.capacity())
                    .saturating_add(entry.whitelist.capacity().saturating_mul(size_of::<String>()))
                    .saturating_add(entry.whitelist.iter().fold(0usize, |sum, item| sum.saturating_add(item.capacity())))
            },
        )
    }

    pub(crate) fn apply(&mut self, update: &ObjectMediaUpdate) {
        for (index, entry) in update.faces.iter().enumerate() {
            let index = u8::try_from(index).expect("ObjectMedia face prefix is at most 45 entries");
            match (self.faces.binary_search_by_key(&index, |(face_index, _)| *face_index), entry) {
                (Ok(slot), Some(entry)) => self.faces[slot].1 = entry.clone(),
                (Ok(slot), None) => {
                    self.faces.remove(slot);
                }
                (Err(slot), Some(entry)) => self.faces.insert(slot, (index, entry.clone())),
                (Err(_), None) => {}
            }
        }
        self.version = update.version;
    }
}

impl MediaEntry {
    fn validate(&self) -> Result<(), ObjectMediaError> {
        if self.current_url.len() > MAX_MEDIA_URL_BYTES || self.home_url.len() > MAX_MEDIA_URL_BYTES {
            return Err(ObjectMediaError::UrlTooLong);
        }
        if self.width_pixels > MAX_MEDIA_DIMENSION || self.height_pixels > MAX_MEDIA_DIMENSION {
            return Err(ObjectMediaError::DimensionTooLarge);
        }
        if self.whitelist.len() > MAX_MEDIA_WHITELIST_ENTRIES
            || self.whitelist.iter().fold(0usize, |total, item| total.saturating_add(item.len())) > MAX_MEDIA_WHITELIST_BYTES
        {
            return Err(ObjectMediaError::WhitelistTooLarge);
        }
        if self.perms_interact & !MEDIA_PERMISSION_MASK != 0 || self.perms_control & !MEDIA_PERMISSION_MASK != 0 {
            return Err(ObjectMediaError::InvalidPermissions);
        }
        Ok(())
    }
}

/// Why a bounded ObjectMedia value could not be constructed or applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectMediaError {
    /// More than 45 TextureEntry faces were supplied.
    TooManyFaces,
    /// A media URL exceeds its 1,024-byte protocol limit.
    UrlTooLong,
    /// A whitelist exceeds its entry or aggregate byte limit.
    WhitelistTooLarge,
    /// A pixel dimension exceeds 2,048.
    DimensionTooLarge,
    /// A permission mask contains unsupported bits.
    InvalidPermissions,
}

impl Display for ObjectMediaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyFaces => f.write_str("too many media faces"),
            Self::UrlTooLong => f.write_str("media URL exceeds the byte limit"),
            Self::WhitelistTooLarge => f.write_str("media whitelist exceeds its entry or byte limit"),
            Self::DimensionTooLarge => f.write_str("media dimensions exceed the pixel limit"),
            Self::InvalidPermissions => f.write_str("media permissions contain unsupported bits"),
        }
    }
}

impl Error for ObjectMediaError {}
