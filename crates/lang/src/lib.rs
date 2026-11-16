mod check;
mod highlight;
mod parse;
mod print;
mod reference;
mod source;

pub use check::{check, format};
pub use highlight::{Highlight, TokenClass, highlight};
pub use parse::{MAX_SOURCE_BYTES, Parsed, literal_value, parse};
pub use print::print;
pub use reference::{
    EXAMPLE, EXPRESSIONS, EXPRESSIONS_INTRODUCTION, EXPRESSIONS_NOTE, FILE_EXTENSION, INTRODUCTION,
    KEYWORDS_INTRODUCTION, STATEMENTS, reference, type_description,
};
pub use source::{LineColumn, LineIndex, SourceMap, TextDiagnostic};
