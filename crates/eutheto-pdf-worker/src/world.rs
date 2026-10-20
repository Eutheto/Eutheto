// SPDX-License-Identifier: Apache-2.0
//! The fixed template and embedded fonts are the entire Typst world.

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Duration, Value};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

pub const TEMPLATE: &str = include_str!("report.typ");
pub const FONT_BYTES: [&[u8]; 5] = [
    include_bytes!("../fonts/LibertinusSerif-Regular.otf"),
    include_bytes!("../fonts/LibertinusSerif-Bold.otf"),
    include_bytes!("../fonts/LibertinusSerif-Italic.otf"),
    include_bytes!("../fonts/LibertinusSerif-BoldItalic.otf"),
    include_bytes!("../fonts/DejaVuSansMono.ttf"),
];

pub struct ReportWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    source: Source,
}

impl ReportWorld {
    pub fn new(model: Vec<u8>) -> Result<Self, &'static str> {
        let mut inputs = Dict::new();
        inputs.insert("report".into(), Value::Bytes(Bytes::new(model)));
        let fonts = FONT_BYTES
            .iter()
            .map(|bytes| Font::new(Bytes::new(*bytes), 0).ok_or("PDF_INVALID_FONT"))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            library: LazyHash::new(Library::builder().with_inputs(inputs).build()),
            book: LazyHash::new(FontBook::from_fonts(&fonts)),
            fonts,
            source: Source::detached(TEMPLATE),
        })
    }

    pub fn supports(&self, character: char) -> bool {
        character.is_ascii()
            || self
                .fonts
                .iter()
                .any(|font| font.info().coverage.contains(u32::from(character)))
    }
}

impl World for ReportWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.source.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.source.id() {
            Ok(Bytes::from_string(self.source.text().to_owned()))
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

    #[test]
    fn only_fixed_template_and_embedded_fonts_are_available()
    -> Result<(), Box<dyn std::error::Error>> {
        let world = ReportWorld::new(b"{}".to_vec())?;
        let other =
            RootedPath::new(VirtualRoot::Project, VirtualPath::new("private.json")?).intern();
        assert!(matches!(world.file(other), Err(FileError::AccessDenied)));
        assert!(matches!(world.source(other), Err(FileError::AccessDenied)));
        assert_eq!(world.source(world.main())?.text(), TEMPLATE);
        assert_eq!(world.file(world.main())?.as_slice(), TEMPLATE.as_bytes());
        assert!(world.font(FONT_BYTES.len()).is_none());
        assert!(world.today(None).is_none());
        Ok(())
    }
}
