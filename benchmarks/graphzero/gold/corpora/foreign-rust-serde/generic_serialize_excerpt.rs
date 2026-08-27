// Foreign Rust fixture: serde-style generic serialization dispatch.
use core::fmt::Write;

pub trait Serializer {
    type Ok;
    fn serialize_str(&mut self, value: &str) -> Self::Ok;
}

pub trait Serialize {
    fn serialize<S: Serializer>(&self, serializer: &mut S) -> S::Ok;
}

pub struct Tag(String);

impl Serialize for Tag {
    fn serialize<S: Serializer>(&self, serializer: &mut S) -> S::Ok {
        serializer.serialize_str(&self.0)
    }
}

pub struct JsonSerializer {
    out: String,
}

impl Serializer for JsonSerializer {
    type Ok = ();

    fn serialize_str(&mut self, value: &str) -> Self::Ok {
        let _ = self.out.write_str(value);
    }
}

pub fn write_tag(tag: &Tag, serializer: &mut JsonSerializer) {
    tag.serialize(serializer);
}

fn unused_escape(value: &str) -> String {
    value.trim().to_string()
}
