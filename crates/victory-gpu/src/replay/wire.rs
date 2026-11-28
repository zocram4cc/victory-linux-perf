//! What a capture holds and how it is written: plain little-endian bytes,
//! fields in order. Read on the machine that wrote it; nothing here is kept
//! compatible between versions beyond the number in [`VERSION`].

/// Changes with every change of the layout.
pub const VERSION: u32 = 1;
/// The first bytes of `frame.bin`.
pub const MAGIC: [u8; 8] = *b"VICFRAME";

/// Why bytes could not be read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bad(pub &'static str);

impl std::fmt::Display for Bad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the capture cannot be read: {}", self.0)
    }
}

impl std::error::Error for Bad {}

/// Bytes being read.
pub struct Dec<'a> {
    pub bytes: &'a [u8],
}

impl<'a> Dec<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Dec { bytes }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Bad> {
        if self.bytes.len() < n {
            return Err(Bad("it ends too early"));
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }
}

/// A value that can be written and read back.
pub trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(from: &mut Dec) -> Result<Self, Bad>;
}

macro_rules! numbers {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn get(from: &mut Dec) -> Result<Self, Bad> {
                let bytes = from.take(size_of::<$t>())?;
                Ok(<$t>::from_le_bytes(bytes.try_into().map_err(|_| Bad("a number"))?))
            }
        }
    )*};
}
numbers!(u8, u16, u32, i32, u64, f32);

impl Wire for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        Ok(u8::get(from)? != 0)
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        for x in self {
            x.put(out);
        }
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        let n = u32::get(from)? as usize;
        // At least a byte each: a wild count is refused, not allocated.
        if n > from.bytes.len() {
            return Err(Bad("a list longer than the file"));
        }
        (0..n).map(|_| T::get(from)).collect()
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        self.is_some().put(out);
        if let Some(x) = self {
            x.put(out);
        }
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        Ok(if bool::get(from)? {
            Some(T::get(from)?)
        } else {
            None
        })
    }
}

impl<T: Wire + Copy + Default, const N: usize> Wire for [T; N] {
    fn put(&self, out: &mut Vec<u8>) {
        for x in self {
            x.put(out);
        }
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        let mut all = [T::default(); N];
        for x in &mut all {
            *x = T::get(from)?;
        }
        Ok(all)
    }
}

impl Wire for String {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        let n = u32::get(from)? as usize;
        String::from_utf8(from.take(n)?.to_vec()).map_err(|_| Bad("a name that is not text"))
    }
}

/// Bytes copied whole.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Blob(pub Vec<u8>);

impl Wire for Blob {
    fn put(&self, out: &mut Vec<u8>) {
        (self.0.len() as u64).put(out);
        out.extend_from_slice(&self.0);
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        let n = u64::get(from)? as usize;
        Ok(Blob(from.take(n)?.to_vec()))
    }
}

macro_rules! tuples {
    ($($t:ident $i:tt),*) => {
        impl<$($t: Wire),*> Wire for ($($t,)*) {
            fn put(&self, out: &mut Vec<u8>) {
                $(self.$i.put(out);)*
            }
            fn get(from: &mut Dec) -> Result<Self, Bad> {
                Ok(($($t::get(from)?,)*))
            }
        }
    };
}
tuples!(A 0, B 1);
tuples!(A 0, B 1, C 2);

macro_rules! wire_struct {
    ($(#[$meta:meta])* $name:ident { $($(#[$fmeta:meta])* $f:ident : $t:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name { $($(#[$fmeta])* pub $f: $t),* }
        impl Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                $(self.$f.put(out);)*
            }
            fn get(from: &mut Dec) -> Result<Self, Bad> {
                Ok($name { $($f: <$t>::get(from)?),* })
            }
        }
    };
}

macro_rules! wire_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $tag:literal $var:ident { $($f:ident : $t:ty),* $(,)? }),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub enum $name { $($(#[$vmeta])* $var { $($f: $t),* }),* }
        impl Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                match self {
                    $($name::$var { $($f),* } => {
                        out.push($tag);
                        $($f.put(out);)*
                    })*
                }
            }
            fn get(from: &mut Dec) -> Result<Self, Bad> {
                Ok(match u8::get(from)? {
                    $($tag => $name::$var { $($f: <$t>::get(from)?),* },)*
                    _ => return Err(Bad("a kind that does not exist")),
                })
            }
        }
    };
}

/// No object.
pub const NONE: u32 = 0;
/// All subresources.
pub const ALL: u32 = u32::MAX;

/// What a descriptor slot holds: the view last made in it. `res` and
/// `counter` are object ids; `desc` the view's description as D3D12 lays it
/// out (a sampler's is the longest: 52 bytes). In a capture file a constant
/// buffer view's buffer is in `res` and, in place of the address, the
/// offset in it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot {
    pub kind: u8,
    /// Whether a description was given.
    pub described: u8,
    pub pad: u16,
    pub res: u32,
    pub counter: u32,
    pub desc: [u8; 52],
}

/// Kinds of [`Slot`].
pub mod slot {
    pub const EMPTY: u8 = 0;
    pub const CBV: u8 = 1;
    pub const SRV: u8 = 2;
    pub const UAV: u8 = 3;
    pub const SAMPLER: u8 = 4;
    pub const RTV: u8 = 5;
    pub const DSV: u8 = 6;
}

impl Slot {
    pub const EMPTY: Slot = Slot {
        kind: slot::EMPTY,
        described: 0,
        pad: 0,
        res: NONE,
        counter: NONE,
        desc: [0; 52],
    };

    /// A slot of `kind` with the description at `desc` (`size` bytes), if any.
    ///
    /// # Safety
    /// `desc` is null or points at a description of `size` bytes.
    pub unsafe fn new(kind: u8, res: u32, counter: u32, desc: *const u8, size: usize) -> Slot {
        let mut s = Slot {
            kind,
            res,
            counter,
            ..Slot::EMPTY
        };
        if !desc.is_null() {
            s.described = 1;
            // SAFETY: the caller's description, at most 52 bytes of it.
            unsafe { std::ptr::copy_nonoverlapping(desc, s.desc.as_mut_ptr(), size.min(52)) };
        }
        s
    }

    /// A constant buffer view's address (or its offset, in a file).
    pub fn address(&self) -> u64 {
        u64::from_le_bytes([
            self.desc[0],
            self.desc[1],
            self.desc[2],
            self.desc[3],
            self.desc[4],
            self.desc[5],
            self.desc[6],
            self.desc[7],
        ])
    }

    pub fn set_address(&mut self, address: u64) {
        self.desc[..8].copy_from_slice(&address.to_le_bytes());
    }
}

impl Wire for Slot {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(self.kind);
        out.push(self.described);
        self.res.put(out);
        self.counter.put(out);
        // Most descriptions are 40 bytes or less.
        let n = self.desc.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        out.push(n as u8);
        out.extend_from_slice(&self.desc[..n]);
    }
    fn get(from: &mut Dec) -> Result<Self, Bad> {
        let (kind, described) = (u8::get(from)?, u8::get(from)?);
        let (res, counter) = (u32::get(from)?, u32::get(from)?);
        let n = u8::get(from)? as usize;
        if n > 52 {
            return Err(Bad("a view's description"));
        }
        let mut desc = [0; 52];
        desc[..n].copy_from_slice(from.take(n)?);
        Ok(Slot {
            kind,
            described,
            pad: 0,
            res,
            counter,
            desc,
        })
    }
}

wire_enum! {
    /// A GPU address.
    Va {
        0 Null {},
        1 At { res: u32, offset: u64 },
        /// In no buffer the recorder knows.
        2 Unknown { address: u64 },
    }
}

wire_enum! {
    /// One side of a texture copy.
    Loc {
        0 Sub { res: u32, sub: u32 },
        1 Placed { res: u32, offset: u64, format: i32, size: [u32; 3], pitch: u32 },
    }
}

wire_enum! {
    /// One barrier of a `ResourceBarrier` call.
    Barrier {
        0 Transition { flags: i32, res: u32, sub: u32, before: i32, after: i32 },
        1 Alias { flags: i32, before: u32, after: u32 },
        2 Uav { flags: i32, res: u32 },
    }
}

wire_enum! {
    /// A command of a list, objects by id, views by what they hold.
    Cmd {
        1 Draw { vertices: u32, instances: u32, first: u32, first_instance: u32 },
        2 DrawIndexed { indices: u32, instances: u32, first: u32, base: i32, first_instance: u32 },
        3 Dispatch { groups: [u32; 3] },
        4 DispatchMesh { groups: [u32; 3] },
        5 CopyBuffer { dst: u32, dst_at: u64, src: u32, src_at: u64, len: u64 },
        6 CopyTexture { dst: Loc, at: [u32; 3], src: Loc, part: Option<[u32; 6]> },
        7 CopyResource { dst: u32, src: u32 },
        8 Resolve { dst: u32, dst_sub: u32, src: u32, src_sub: u32, format: i32 },
        9 ResolveRegion { dst: u32, dst_sub: u32, at: [u32; 2], src: u32, src_sub: u32, part: Option<[i32; 4]>, format: i32, mode: i32 },
        10 Topology { kind: i32 },
        11 Viewports { all: Vec<[f32; 6]> },
        12 Scissors { all: Vec<[i32; 4]> },
        13 BlendFactor { factor: Option<[f32; 4]> },
        14 StencilRef { value: u32 },
        15 Pipeline { pso: u32 },
        16 Barriers { all: Vec<Barrier> },
        17 Heaps { heaps: Vec<u32> },
        18 RootSignature { graphics: bool, root: u32 },
        19 Table { graphics: bool, param: u32, heap: u32, slot: u32 },
        20 Constants { graphics: bool, param: u32, offset: u32, values: Vec<u32> },
        21 RootView { graphics: bool, kind: u8, param: u32, va: Va },
        22 IndexBuffer { view: Option<(Va, u32, i32)> },
        23 VertexBuffers { start: u32, count: u32, views: Option<Vec<(Va, u32, u32)>> },
        24 Targets { colors: Vec<Slot>, depth: Option<Slot> },
        25 ClearDepth { view: Slot, flags: i32, depth: f32, stencil: u8, rects: Vec<[i32; 4]> },
        26 ClearTarget { view: Slot, color: [f32; 4], rects: Vec<[i32; 4]> },
        27 Discard { res: u32, rects: Vec<[i32; 4]>, subs: Option<[u32; 2]> },
        28 BeginQuery { heap: u32, kind: i32, index: u32 },
        29 EndQuery { heap: u32, kind: i32, index: u32 },
        30 ResolveQuery { heap: u32, kind: i32, first: u32, count: u32, dst: u32, at: u64 },
        31 Indirect { signature: u32, most: u32, args: u32, at: u64, count: u32, count_at: u64 },
        32 WriteImmediate { all: Vec<(Va, u32, i32)> },
        33 DepthBounds { min: f32, max: f32 },
    }
}

/// The highest kind number of [`Cmd`].
#[cfg(test)]
const LAST_CMD: u8 = 33;

impl Cmd {
    /// The commands of a list's bytes.
    pub fn all(bytes: &[u8]) -> Result<Vec<Cmd>, Bad> {
        let mut from = Dec::new(bytes);
        let mut all = Vec::new();
        while !from.bytes.is_empty() {
            all.push(Cmd::get(&mut from)?);
        }
        Ok(all)
    }

    /// Whether it makes the card work (and is timed on its own).
    pub fn works(&self) -> bool {
        self.runs_shaders()
            || matches!(
                self,
                Cmd::CopyBuffer { .. }
                    | Cmd::CopyTexture { .. }
                    | Cmd::CopyResource { .. }
                    | Cmd::Resolve { .. }
                    | Cmd::ResolveRegion { .. }
                    | Cmd::ClearDepth { .. }
                    | Cmd::ClearTarget { .. }
                    | Cmd::Discard { .. }
                    | Cmd::ResolveQuery { .. }
                    | Cmd::WriteImmediate { .. }
            )
    }

    /// Whether it runs the pipeline bound.
    pub fn runs_shaders(&self) -> bool {
        matches!(
            self,
            Cmd::Draw { .. }
                | Cmd::DrawIndexed { .. }
                | Cmd::Dispatch { .. }
                | Cmd::DispatchMesh { .. }
                | Cmd::Indirect { .. }
        )
    }
}

wire_struct! {
    HeapProps { kind: i32, cpu: i32, pool: i32 }
}

wire_struct! {
    /// A resource description (`D3D12_RESOURCE_DESC`).
    Desc {
        dimension: i32,
        alignment: u64,
        width: u64,
        height: u32,
        depth: u16,
        mips: u16,
        format: i32,
        samples: [u32; 2],
        layout: i32,
        flags: i32,
    }
}

wire_struct! {
    Heap { id: u32, size: u64, props: HeapProps, alignment: u64, flags: i32 }
}

wire_enum! {
    /// How a resource got its memory.
    Place {
        0 Committed { props: HeapProps, flags: i32 },
        1 Placed { heap: u32, offset: u64 },
        /// A swap chain's buffer.
        2 BackBuffer {},
    }
}

wire_struct! {
    Resource {
        id: u32,
        place: Place,
        desc: Desc,
        /// The state it was made in.
        made_in: i32,
        clear: Option<[u32; 5]>,
        /// Its states at the start of the frame: one, or one for each
        /// subresource. Buffers start in the common state.
        state: Vec<u32>,
        /// Not the one in use where placed resources overlap: its contents
        /// are not in the capture.
        idle: bool,
        /// Made after the frame started.
        late: bool,
        name: String,
    }
}

wire_enum! {
    /// A part of a pipeline description.
    Part {
        0 Root { root: u32 },
        /// Shader code: the number of its blob ([`Frame::codes`]).
        1 Code { kind: i32, blob: u32 },
        /// Plain data.
        2 Plain { kind: i32, data: Blob },
        3 Layout { elements: Vec<(String, [u32; 6])> },
    }
}

wire_struct! {
    Pipeline { id: u32, parts: Vec<Part>, name: String }
}

wire_struct! {
    /// Bytes in `shaders.bin`.
    Code { at: u64, len: u64, hash: [u8; 32] }
}

wire_struct! {
    Signature { id: u32, stride: u32, args: Vec<[u32; 4]>, root: u32 }
}

wire_struct! {
    DescriptorHeap { id: u32, kind: i32, count: u32, flags: i32, runs: Vec<(u32, u32, Slot)> }
}

wire_struct! {
    QueryHeap { id: u32, kind: i32, count: u32 }
}

wire_struct! {
    List { kind: i32, commands: Blob }
}

wire_struct! {
    /// Bytes the CPU (or an earlier submission) put into a buffer the CPU
    /// sees, by the time a submission went to the card (in `deltas.bin`).
    Delta { res: u32, at: u64, len: u64, file: u64 }
}

wire_struct! {
    Submit {
        frame: u32,
        /// Lists by number; [`MISSING`] for one that was not recorded.
        lists: Vec<u32>,
        deltas: Vec<Delta>,
        /// Descriptor slots written since the submission before.
        views: Vec<(u32, u32, Slot)>,
    }
}

/// In [`Submit::lists`].
pub const MISSING: u32 = u32::MAX;

wire_struct! {
    /// Contents at the start of the frame, in `contents.bin`: of a buffer,
    /// `len` bytes from `first`; of a texture, `count` subresources from
    /// `first`, laid out as `GetCopyableFootprints` lays them out.
    Content {
        res: u32,
        first: u64,
        count: u32,
        len: u64,
        /// Where in the file; [`ZERO`] if all bytes are zero.
        file: u64,
    }
}

/// In [`Content::file`].
pub const ZERO: u64 = u64::MAX;

wire_struct! {
    /// A frame's picture: a texture as it was when presented (in
    /// `present.bin`).
    Present { submits: u32, res: u32, width: u32, height: u32, format: i32, pitch: u32, file: u64 }
}

wire_struct! {
    Frame {
        adapter: String,
        vendor: u32,
        device: u32,
        /// What the recorder could not record, and how it went.
        notes: Vec<String>,
        heaps: Vec<Heap>,
        resources: Vec<Resource>,
        roots: Vec<(u32, Blob)>,
        codes: Vec<Code>,
        pipelines: Vec<Pipeline>,
        signatures: Vec<Signature>,
        descriptor_heaps: Vec<DescriptorHeap>,
        query_heaps: Vec<QueryHeap>,
        lists: Vec<List>,
        submits: Vec<Submit>,
        contents: Vec<Content>,
        presents: Vec<Present>,
    }
}

impl Frame {
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut out = MAGIC.to_vec();
        VERSION.put(&mut out);
        self.put(&mut out);
        std::fs::write(path, out)
    }

    pub fn read(path: &std::path::Path) -> anyhow::Result<Frame> {
        let bytes = std::fs::read(path)?;
        let mut from = Dec::new(&bytes);
        if from.take(8)? != MAGIC {
            return Err(Bad("it is not a frame").into());
        }
        if u32::get(&mut from)? != VERSION {
            return Err(Bad("it was made by another version of the recorder").into());
        }
        let frame = Frame::get(&mut from)?;
        if !from.bytes.is_empty() {
            return Err(Bad("bytes after its end").into());
        }
        Ok(frame)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn there_and_back<T: Wire + PartialEq + std::fmt::Debug>(x: &T) {
        let mut out = Vec::new();
        x.put(&mut out);
        let mut from = Dec::new(&out);
        assert_eq!(&T::get(&mut from).unwrap(), x);
        assert!(from.bytes.is_empty(), "bytes left over");
        // Cut anywhere, it is refused and does not panic.
        for n in 0..out.len() {
            assert!(T::get(&mut Dec::new(&out[..n])).is_err());
        }
    }

    fn view() -> Slot {
        let mut desc = [0; 52];
        desc[3] = 7;
        desc[51] = 9;
        Slot {
            kind: slot::SAMPLER,
            described: 1,
            pad: 0,
            res: 4,
            counter: 5,
            desc,
        }
    }

    fn at() -> Va {
        Va::At {
            res: 3,
            offset: 1 << 40,
        }
    }

    pub(crate) fn every_command() -> Vec<Cmd> {
        let placed = Loc::Placed {
            res: 5,
            offset: 512,
            format: 28,
            size: [4, 5, 6],
            pitch: 256,
        };
        let rects = vec![[1, 2, 3, 4]];
        vec![
            Cmd::Draw {
                vertices: 1,
                instances: 2,
                first: 3,
                first_instance: 4,
            },
            Cmd::DrawIndexed {
                indices: 1,
                instances: 2,
                first: 3,
                base: -4,
                first_instance: 5,
            },
            Cmd::Dispatch { groups: [1, 2, 3] },
            Cmd::DispatchMesh { groups: [4, 5, 6] },
            Cmd::CopyBuffer {
                dst: 1,
                dst_at: 2,
                src: 3,
                src_at: 4,
                len: 5,
            },
            Cmd::CopyTexture {
                dst: Loc::Sub { res: 1, sub: 2 },
                at: [3, 4, 5],
                src: placed,
                part: Some([1, 2, 3, 4, 5, 6]),
            },
            Cmd::CopyResource { dst: 1, src: 2 },
            Cmd::Resolve {
                dst: 1,
                dst_sub: 2,
                src: 3,
                src_sub: 4,
                format: 28,
            },
            Cmd::ResolveRegion {
                dst: 1,
                dst_sub: 2,
                at: [3, 4],
                src: 5,
                src_sub: 6,
                part: Some([0, 1, 2, 3]),
                format: 28,
                mode: 1,
            },
            Cmd::Topology { kind: 4 },
            Cmd::Viewports {
                all: vec![[0.0, 1.0, 2.0, 3.0, 0.5, 1.0]; 2],
            },
            Cmd::Scissors { all: rects.clone() },
            Cmd::BlendFactor {
                factor: Some([0.1, 0.2, 0.3, 0.4]),
            },
            Cmd::BlendFactor { factor: None },
            Cmd::StencilRef { value: 255 },
            Cmd::Pipeline { pso: 9 },
            Cmd::Barriers {
                all: vec![
                    Barrier::Transition {
                        flags: 1,
                        res: 2,
                        sub: ALL,
                        before: 4,
                        after: 8,
                    },
                    Barrier::Alias {
                        flags: 0,
                        before: 1,
                        after: 2,
                    },
                    Barrier::Uav { flags: 0, res: 0 },
                ],
            },
            Cmd::Heaps { heaps: vec![1, 2] },
            Cmd::RootSignature {
                graphics: true,
                root: 3,
            },
            Cmd::Table {
                graphics: false,
                param: 1,
                heap: 2,
                slot: 1 << 20,
            },
            Cmd::Constants {
                graphics: true,
                param: 1,
                offset: 2,
                values: vec![3, 4, 5],
            },
            Cmd::RootView {
                graphics: true,
                kind: slot::CBV,
                param: 1,
                va: at(),
            },
            Cmd::IndexBuffer {
                view: Some((at(), 5, 42)),
            },
            Cmd::IndexBuffer { view: None },
            Cmd::VertexBuffers {
                start: 1,
                count: 1,
                views: Some(vec![(Va::Unknown { address: 5 }, 6, 7)]),
            },
            Cmd::VertexBuffers {
                start: 1,
                count: 3,
                views: None,
            },
            Cmd::Targets {
                colors: vec![view(); 3],
                depth: Some(view()),
            },
            Cmd::ClearDepth {
                view: view(),
                flags: 3,
                depth: 0.5,
                stencil: 7,
                rects: rects.clone(),
            },
            Cmd::ClearTarget {
                view: view(),
                color: [1.0, 2.0, 3.0, 4.0],
                rects: Vec::new(),
            },
            Cmd::Discard {
                res: 1,
                rects,
                subs: Some([0, 2]),
            },
            Cmd::BeginQuery {
                heap: 1,
                kind: 0,
                index: 2,
            },
            Cmd::EndQuery {
                heap: 1,
                kind: 2,
                index: 3,
            },
            Cmd::ResolveQuery {
                heap: 1,
                kind: 2,
                first: 3,
                count: 4,
                dst: 5,
                at: 6,
            },
            Cmd::Indirect {
                signature: 1,
                most: 2,
                args: 3,
                at: 4,
                count: 5,
                count_at: 6,
            },
            Cmd::WriteImmediate {
                all: vec![(at(), 1, 2), (Va::Null {}, 3, 4)],
            },
            Cmd::DepthBounds {
                min: 0.25,
                max: 0.75,
            },
        ]
    }

    #[test]
    fn every_command_comes_back() {
        let all = every_command();
        let mut bytes = Vec::new();
        for c in &all {
            there_and_back(c);
            c.put(&mut bytes);
        }
        assert_eq!(Cmd::all(&bytes).unwrap(), all);
        assert!(Cmd::all(&[250]).is_err());
        // Every kind there is stands in the list above.
        let kinds: std::collections::BTreeSet<u8> = all
            .iter()
            .map(|c| {
                let mut out = Vec::new();
                c.put(&mut out);
                out[0]
            })
            .collect();
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            (1..=LAST_CMD).collect::<Vec<u8>>()
        );
        assert!(
            Cmd::all(&[LAST_CMD + 1]).is_err(),
            "a kind was added: bring LAST_CMD and the list above up to date"
        );
    }

    #[test]
    fn a_frame_comes_back() {
        let props = HeapProps {
            kind: 4,
            cpu: 3,
            pool: 1,
        };
        let desc = Desc {
            dimension: 3,
            alignment: 65536,
            width: 1 << 33,
            height: 5,
            depth: 6,
            mips: 7,
            format: 28,
            samples: [1, 0],
            layout: 1,
            flags: 4,
        };
        let resource = Resource {
            id: 2,
            place: Place::Placed {
                heap: 1,
                offset: 65536,
            },
            desc,
            made_in: 4,
            clear: Some([28, 1, 2, 3, 4]),
            state: vec![1, 2, 3],
            idle: false,
            late: true,
            name: "a texture".into(),
        };
        let frame = Frame {
            adapter: "a card".into(),
            vendor: 0x1002,
            device: 0x744c,
            notes: vec!["nothing".into()],
            heaps: vec![Heap {
                id: 1,
                size: 64 << 20,
                props: props.clone(),
                alignment: 65536,
                flags: 0x84,
            }],
            resources: vec![
                resource.clone(),
                Resource {
                    place: Place::Committed { props, flags: 8 },
                    ..resource.clone()
                },
                Resource {
                    place: Place::BackBuffer {},
                    clear: None,
                    ..resource
                },
            ],
            roots: vec![(3, Blob(vec![1, 2, 3]))],
            codes: vec![Code {
                at: 1,
                len: 2,
                hash: [7; 32],
            }],
            pipelines: vec![Pipeline {
                id: 4,
                parts: vec![
                    Part::Root { root: 3 },
                    Part::Code { kind: 25, blob: 0 },
                    Part::Plain {
                        kind: 8,
                        data: Blob(vec![9; 20]),
                    },
                    Part::Layout {
                        elements: vec![("POSITION".into(), [1, 2, 3, 4, 5, 6])],
                    },
                ],
                name: "a pipeline".into(),
            }],
            signatures: vec![Signature {
                id: 5,
                stride: 12,
                args: vec![[10, 0, 0, 0]],
                root: 0,
            }],
            descriptor_heaps: vec![DescriptorHeap {
                id: 6,
                kind: 0,
                count: 10,
                flags: 1,
                runs: vec![(0, 9, Slot::EMPTY), (9, 1, view())],
            }],
            query_heaps: vec![QueryHeap {
                id: 7,
                kind: 1,
                count: 8,
            }],
            lists: vec![List {
                kind: 0,
                commands: Blob(vec![1, 2]),
            }],
            submits: vec![Submit {
                frame: 0,
                lists: vec![0, MISSING],
                deltas: vec![Delta {
                    res: 2,
                    at: 4096,
                    len: 8192,
                    file: 0,
                }],
                views: vec![(6, 3, view())],
            }],
            contents: vec![Content {
                res: 2,
                first: 0,
                count: 3,
                len: 100,
                file: ZERO,
            }],
            presents: vec![Present {
                submits: 1,
                res: 2,
                width: 4,
                height: 5,
                format: 28,
                pitch: 256,
                file: 0,
            }],
        };
        there_and_back(&frame);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.bin");
        frame.write(&path).unwrap();
        assert_eq!(Frame::read(&path).unwrap(), frame);
        std::fs::write(&path, b"VICFRAMEnothing at all").unwrap();
        assert!(Frame::read(&path).is_err());
    }
}
