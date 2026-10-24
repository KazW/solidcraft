//! A design can use the user parameters of other designs (Link Parameters): `linked_designs`
//! lists each linked design's path (relative to the design's folder), which of its parameters come
//! in (`scope`) and their values as last read. Older designs need no transformation: they link no
//! parameters. The last release opens a design saved with this with a warning about the newer
//! format number, reads what it can and keeps `linked_designs` as it was
//! (`crates/doc/src/newer_file.rs`).
