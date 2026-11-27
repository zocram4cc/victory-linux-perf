//! A stand-in for the indirect light the title traces rays for.
//!
//! With the ray-tracing structure builder off (`crate::pipeline::builder_runs`)
//! the title's rays find nothing. Its program that resolves the diffuse light
//! of its screen probes (`CS_ScreenProbes_TemporalResolve_*`) then leaves the
//! sky's light outdoors, nothing indoors (walls are black where no lamp or
//! sun reaches) and, in blocks of the picture's tiles, what the probes'
//! memory held. Right after that program the light is raised to an estimate
//! and bounded by a multiple of it.
//!
//! That a stand-in can be put there, and in which of the program's images,
//! is beloooo's find (VIctory-Belobug-AMD, 06031237, 4f45ab88, cd9f78ce,
//! GPL-3.0-or-later), which puts a constant there, shaded in creases from the
//! scene's depth. The estimate here is worked out from the frame instead, so
//! it follows the hour, the room and its lamps by itself: the light the
//! visible surfaces send out (the title keeps it for its own screen traces:
//! `CS_CacheScreenSpaceRadiance`, the frame before) is averaged in 64x36
//! cells, and every point gets the mean of the cells weighted by the share of
//! its surroundings they cover, as a directional value (first-order harmonics
//! at 160x90 points, taken between them by depth).
//!
//! What it assumes: what is not on screen sends out what is (with a slow
//! mean of the last seconds, so that turning into a dark corner is not black
//! at once). No occlusion but the creases'.
//!
//! `VICTORY_AMBIENT`:
//! - unset or `0`/`off`: the title's own output.
//! - a number: that share of the estimate (1: all of it).
//! - `constant:<level>`: the constant, as beloooo's fork has it.
//!
//! `VICTORY_AMBIENT_SHADE=<strength>,<radius in metres>` sets the creases.
//!
//! At a camera cut everything the estimate has is of the shot before: the
//! light the surfaces sent out is the frame before's, and the probes'
//! memory still holds that shot in blocks of tiles. Raised to the old
//! estimate, the first frames of the new shot showed the old one's colours:
//! a sunset's warm veil over a night street, hard pale and orange blocks in
//! the sky, gone two or three frames later. A cut shows as most of the
//! scene's depth changing at once; for the frames after it
//! (`VICTORY_AMBIENT_CUT=<frames>`, 2; 0: not looked for) nothing is raised
//! and what the program left is held to the old estimate's brightness, in
//! grey, and the slow mean starts again from the new shot.
//!
//! The air of a room is lit by the same probes: without rays they hold the
//! sky's light indoors too, a brown veil at sunset. `VICTORY_AMBIENT_AIR`
//! bounds what the air scatters by a multiple of the light around.
//!
//! So are the reflections: without rays a glossy surface indoors mirrors the
//! sky, unoccluded (a floor with a noisy pink and purple glaze, eyes lit with
//! sky indoors). `VICTORY_AMBIENT_REFLECT=<part>` holds what the title's
//! `CS_UpdateHistory` leaves of them to that part of the light around
//! (Neptune_VI (zm0kiemedia), Belobug's pass: there a part of its constant
//! light, here of this estimate). Off unless set; 0.5 is Neptune's.
//!
//! Where that sky comes from: the reflections are traced at half the
//! picture's size. Rays the title's screen-space march resolves
//! (`CS_ResolveScreenSpace`) take the light seen on screen; the others go to
//! the structures, find nothing, and `CS_RayMiss` gives each the sky's light
//! from the sky's cube maps (10 to 30 against 0.01 to 0.1 for the room the
//! eyes and floors are in, and blue: eyes glowing blue-white).
//! `VICTORY_AMBIENT_SPECULAR=<share>[,<sky>]` puts in, right after that
//! program, the same estimate taken along the reflected direction
//! ([`Role::Miss`]; trustno1 2db59d4 and what followed it); the screen-space
//! resolve that comes next still writes what it found over it. Off unless
//! set, and it needs no `VICTORY_AMBIENT`. It acts at the source, on the rays
//! that missed; `VICTORY_AMBIENT_REFLECT` bounds the result afterwards, in
//! the history, hits and misses alike. Either works without the other.

use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

use crate::host::{HostList, host};

/// Programs of the title the stand-in follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// Resolves the diffuse light.
    Resolve,
    /// Keeps the light visible surfaces send out.
    Radiance,
    /// Adds the lamps' light to what the air scatters (the last program
    /// that writes that volume before it is summed up along the view).
    Air,
    /// Writes the title's reflections (`CS_UpdateHistory`), which the
    /// lighting programs read next.
    Reflect,
    /// Gives the reflection rays that found nothing the sky's light
    /// (`CS_RayMiss`, not `CS_RayMiss_Volumetrics` nor the screen probes').
    Miss,
}

/// The program's part in this, by its name in the container.
pub(crate) fn role(container: &[u8]) -> Option<Role> {
    let has = |name: &[u8]| container.windows(name.len()).any(|w| w == name);
    if has(b"CS_ScreenProbes_TemporalResolve") {
        Some(Role::Resolve)
    } else if has(b"CS_CacheScreenSpaceRadiance") {
        Some(Role::Radiance)
    } else if has(b"CS_AccumulateLocalLightInscatteringVolume") {
        Some(Role::Air)
    } else if has(b"CS_UpdateHistory") {
        Some(Role::Reflect)
    } else if is_miss(container) {
        Some(Role::Miss)
    } else {
        None
    }
}

/// `CS_RayMiss_` and the eight hex digits of a variant's hash.
fn is_miss(container: &[u8]) -> bool {
    const NAME: &[u8] = b"CS_RayMiss_";
    container.windows(NAME.len() + 9).any(|w| {
        w.starts_with(NAME) && w[NAME.len()..NAME.len() + 8].iter().all(u8::is_ascii_hexdigit) && w[NAME.len() + 8] == b'_'
    })
}

/// Where the resolve writes the diffuse light (R9G9B9E5 as 32-bit integers)
/// and reads the scene's depth, and where the other writes the light of
/// visible surfaces: root parameter and place in its table, in dwords.
pub(crate) const LIGHT: (u8, u32) = (1, 176);
pub(crate) const DEPTH: (u8, u32) = (0, 464);
pub(crate) const RADIANCE: (u8, u32) = (1, 16);
/// Where the light the air scatters is.
pub(crate) const AIR: (u8, u32) = (1, 32);
/// Where `CS_UpdateHistory` writes the reflections (R9G9B9E5 as 32-bit
/// integers), as in Neptune_VI.
pub(crate) const REFLECTIONS: (u8, u32) = (1, 40);
/// Where the miss program writes the reflected light (R11G11B10, half the
/// picture's size) and where it reads the depth it is given (a 32-bit depth
/// of the picture's size). Variants bind them elsewhere: then they are
/// looked for by kind.
pub(crate) const MISS_LIGHT: (u8, u32) = (1, 104);
pub(crate) const MISS_DEPTH: (u8, u32) = (0, 1000);
/// The miss program's first root constant: which surfaces its rays are of
/// (2 and more: water and others, left as the title has them).
pub(crate) const MISS_OPAQUE: u32 = 0;
pub(crate) const MISS_ALPHA: u32 = 1;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Set {
    Off,
    Estimate(f32),
    Constant(f32),
}

fn parse(set: &str) -> Set {
    let set = set.trim();
    let number = |t: &str| t.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0);
    match set.strip_prefix("constant:") {
        Some(level) => number(level).map_or(Set::Off, Set::Constant),
        None => number(set).map_or(Set::Off, Set::Estimate),
    }
}

fn set() -> Set {
    static SET: OnceLock<Set> = OnceLock::new();
    *SET.get_or_init(|| parse(&std::env::var("VICTORY_AMBIENT").unwrap_or_default()))
}

/// Whether a stand-in is on (for the diffuse light or for reflections):
/// set, and the title's rays find nothing.
pub(crate) fn on() -> bool {
    (set() != Set::Off || specular().is_some()) && !crate::pipeline::builder_runs()
}

/// `VICTORY_AMBIENT_SPECULAR=<share>[,<sky>]`: reflections whose rays found
/// nothing take `share` of the estimate along the reflected direction
/// instead of the sky's light; `sky` (0 to 1, 0 if not given) of them stays
/// the title's sky, made no brighter than the estimate. Unset or 0: the
/// title's sky, as before.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Specular {
    share: f32,
    sky: f32,
}

fn parse_specular(set: &str) -> Option<Specular> {
    let mut parts = set.split(',').map(|t| t.trim().parse::<f32>().ok().filter(|v| v.is_finite()));
    let share = parts.next().flatten().filter(|v| *v > 0.0)?;
    let sky = parts.next().flatten().map_or(0.0, |v| v.clamp(0.0, 1.0));
    Some(Specular { share, sky })
}

fn specular() -> Option<Specular> {
    static SET: OnceLock<Option<Specular>> = OnceLock::new();
    *SET.get_or_init(|| parse_specular(&std::env::var("VICTORY_AMBIENT_SPECULAR").unwrap_or_default()))
}

/// Whether the stand-in for reflections is asked for ([`Specular`]).
pub(crate) fn mirroring() -> bool {
    specular().is_some()
}

/// Whether the stand-in for reflections also takes the alpha layer's rays
/// (`VICTORY_AMBIENT_SPECULAR_ALPHA=1`; off: opaque surfaces only). The
/// alpha layer's pixels are of transparent surfaces (glass, foam, wakes on
/// water) and the depth read is the opaque one behind them: on water the
/// bottom, so the reflected direction is wrong and the estimate dark (dark
/// ragged patches around a boat; trustno1 9f6fb77). For interiors.
pub(crate) fn mirroring_alpha() -> bool {
    static SET: OnceLock<bool> = OnceLock::new();
    *SET.get_or_init(|| std::env::var("VICTORY_AMBIENT_SPECULAR_ALPHA").is_ok_and(|v| v.trim() == "1"))
}

/// What the raise keeps of the light the title's own resolve left
/// (`VICTORY_AMBIENT_LEFTOVER`; the shader reads 0, 1, 2):
/// - unset or `keep` (0): the larger of it and the estimate per channel, its
///   brightness bounded to `most` times the estimate's: its hue stays. Without
///   rays that leftover is whatever the probes' memory held; other trees saw
///   paper glow magenta and a floor take a pink cast with it
///   (Ortomanic/victory-fork bc4fee9, from Neptune-VE).
/// - `hue` (1): its brightness, within the same bound, in the estimate's colour.
/// - `off` (2): the estimate alone.
fn leftover() -> u32 {
    static SET: OnceLock<u32> = OnceLock::new();
    *SET.get_or_init(|| leftover_mode(&std::env::var("VICTORY_AMBIENT_LEFTOVER").unwrap_or_default()))
}

/// [`leftover`] of a switch's value. Pure, for testing.
fn leftover_mode(set: &str) -> u32 {
    match set.trim() {
        s if s.eq_ignore_ascii_case("hue") => 1,
        s if s.eq_ignore_ascii_case("off") || s == "0" => 2,
        _ => 0,
    }
}

/// The most the air scatters, as a multiple of the light around
/// (`VICTORY_AMBIENT_AIR=<multiple>`; unset or 0: what the title has).
fn air() -> f32 {
    static SET: OnceLock<f32> = OnceLock::new();
    *SET.get_or_init(|| {
        let set = std::env::var("VICTORY_AMBIENT_AIR").unwrap_or_default();
        set.trim().parse().ok().filter(|v: &f32| v.is_finite() && *v > 0.0).unwrap_or(0.0)
    })
}

/// The most a reflection may hold, as a part of the light around
/// (`VICTORY_AMBIENT_REFLECT=<part>`; unset or 0: the title's reflections as
/// they are).
fn reflect() -> f32 {
    static SET: OnceLock<f32> = OnceLock::new();
    *SET.get_or_init(|| reflect_part(std::env::var("VICTORY_AMBIENT_REFLECT").ok().as_deref()))
}

/// Whether reflections are held at all ([`reflect`]).
pub(crate) fn holding() -> bool {
    reflect() > 0.0
}

/// [`reflect`] for a value of `VICTORY_AMBIENT_REFLECT`.
fn reflect_part(set: Option<&str>) -> f32 {
    set.and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite() && *v > 0.0).map_or(0.0, |v| v.min(16.0))
}

/// Frames after a camera cut in which the estimate, which is of the shot
/// before, is not trusted (`VICTORY_AMBIENT_CUT=<frames>`; 2 unset, 0 off).
fn cut() -> f32 {
    static SET: OnceLock<f32> = OnceLock::new();
    *SET.get_or_init(|| cut_frames(std::env::var("VICTORY_AMBIENT_CUT").ok().as_deref()))
}

/// [`cut`] for a value of `VICTORY_AMBIENT_CUT`.
fn cut_frames(set: Option<&str>) -> f32 {
    match set.map(str::trim) {
        None | Some("") => 2.0,
        Some(v) => v.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0).map_or(2.0, |v| v.min(30.0).floor()),
    }
}

/// Strength and radius of the creases' shade.
fn shade() -> [f32; 2] {
    static SET: OnceLock<[f32; 2]> = OnceLock::new();
    *SET.get_or_init(|| {
        let mut shade = [1.0, 0.6];
        let set = std::env::var("VICTORY_AMBIENT_SHADE").unwrap_or_default();
        for (to, v) in shade.iter_mut().zip(set.split(',')) {
            if let Ok(v) = v.trim().parse() {
                *to = v;
            }
        }
        shade
    })
}

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=14, b0), DescriptorTable(SRV(t0)), DescriptorTable(UAV(u0)), UAV(u1), UAV(u2)"
#define CX 64
#define CY 36
#define CELLS (CX * CY)
#define PX 160
#define PY 90
// The cells' positions the frame before, then the cut's state: frames of
// the cut's window left, and the share of cells whose depth changed.
#define PREV (2 * CELLS + 1)
#define CUT_STATE (3 * CELLS + 1)
cbuffer Set : register(b0) {
    float level; float constant; float strength; float radius;
    float near; float tan_y; uint width; uint height;
    float follow; float most; float air; float cut;
    float held; uint leftover;
};
Texture2D<float> depth : register(t0);
// Cells: the light they send out (then, at CELLS, the slow mean), from
// CELLS + 1 on where they are, from PREV on where they were the frame
// before, and at CUT_STATE the cut's state.
RWStructuredBuffer<float4> cells : register(u1);
// Three a point: red, green and blue, each a mean and a direction.
RWStructuredBuffer<float4> points : register(u2);
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

// R9G9B9E5, as the title keeps light in 32-bit integers.
float3 unpack(uint v) {
    return float3(v & 511, (v >> 9) & 511, (v >> 18) & 511) * exp2(float(v >> 27) - 24.0);
}
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float top = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(top, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (top * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}

// The point seen at a texel, in metres from the eye (x right, y up, z away).
float3 seen(int2 p, float2 size) {
    float d = depth.Load(int3(clamp(p, int2(0, 0), int2(size) - 1), 0));
    float z = min(near / max(d, 1e-7), 1000.0);
    float2 ndc = (float2(p) + 0.5) / size * 2.0 - 1.0;
    return float3(ndc.x * tan_y * size.x / size.y * z, -ndc.y * tan_y * z, z);
}

#ifdef KEEP
RWTexture2D<float3> radiance : register(u0);
[RootSignature(RS)]
[numthreads(8, 4, 1)]
void keep(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    radiance.GetDimensions(w, h);
    float3 sum = 0;
    for (uint y = 0; y < 5; y++) for (uint x = 0; x < 5; x++) {
        uint2 p = uint2((float2(at.xy) + (float2(x, y) + 0.5) / 5.0) * float2(w, h) / float2(CX, CY));
        float3 c = max(radiance[p], 0.0);
        c = any(isnan(c)) || any(isinf(c)) ? 0.0 : c;
        // Lamps and glints count, but not for everything.
        sum += c / max(1.0, dot(c, LUMA) / 20.0);
    }
    cells[at.y * CX + at.x] = float4(sum / 25.0, 1.0);
}
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void mean() {
    float3 sum = 0;
    for (uint i = 0; i < CELLS; i++) sum += cells[i].rgb;
    float4 was = cells[CELLS];
    float3 now = sum / CELLS;
    // After a cut the shot before's mean counts for nothing.
    bool again = was.a <= 0.0 || cells[CUT_STATE].x > 0.0;
    cells[CELLS] = float4(again ? now : lerp(was.rgb, now, follow), 1.0);
}
#endif

#ifdef PLACE
[RootSignature(RS)]
[numthreads(8, 4, 1)]
void place(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    float3 p = seen(int2((float2(at.xy) + 0.5) * size / float2(CX, CY)), size);
    uint i = at.y * CX + at.x;
    // (A negative `cut`: the cells were placed in this frame already, by the
    // stand-in for reflections; where they were the frame before stays, and
    // `gather` does not look for a cut again.)
    if (cut >= 0.0) cells[PREV + i] = cells[CELLS + 1 + i];
    cells[CELLS + 1 + i] = float4(p, p.z * p.z);
}
#endif

#ifdef GATHER
[RootSignature(RS)]
[numthreads(8, 2, 1)]
void gather(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    // A cut: most cells' depth changed by more than a third since the frame
    // before (a pan moves some, a cut all). One thread decides for all.
    if (cut > 0.0 && at.x == 0 && at.y == 0) {
        float moved = 0, valid = 0;
        for (uint i = 0; i < CELLS; i++) {
            float4 n = cells[CELLS + 1 + i], o = cells[PREV + i];
            if (n.w > 0.0 && o.w > 0.0) {
                valid += 1;
                moved += abs(n.z - o.z) > 0.33 * max(n.z, o.z) ? 1 : 0;
            }
        }
        float share = moved / max(valid, 1.0);
        float left = cells[CUT_STATE].x;
        cells[CUT_STATE] = float4(share > 0.5 ? cut : max(left - 1.0, 0.0), share, 0, 0);
    }
    float3 p = seen(int2((float2(at.xy) + 0.5) * size / float2(PX, PY)), size);
    float r2 = 0.25 + 0.0025 * p.z * p.z;
    float whole = 0;
    float3 toward = 0, light = 0, red = 0, green = 0, blue = 0;
    for (uint i = 0; i < CELLS; i++) {
        float4 c = cells[CELLS + 1 + i];
        float3 L = cells[i].rgb;
        float3 d = c.xyz - p;
        float dd = dot(d, d);
        float w = c.w / (dd + r2);
        float3 dir = d / sqrt(dd + 0.25 * r2);
        whole += w;
        toward += w * dir;
        light += w * L;
        red += w * L.r * dir;
        green += w * L.g * dir;
        blue += w * L.b * dir;
    }
    whole = max(whole, 1e-6);
    light /= whole;
    toward /= whole;
    uint at0 = (at.y * PX + at.x) * 3;
    points[at0] = float4(red / whole - light.r * toward, light.r);
    points[at0 + 1] = float4(green / whole - light.g * toward, light.g);
    points[at0 + 2] = float4(blue / whole - light.b * toward, light.b);
}
#endif

#ifdef MISS
RWTexture2D<float3> missed : register(u0);
// Each texel of the reflection rays' image (half the picture's size): the
// light the points around send along the reflected direction, as `raise`
// takes it along the normal; `level` is the share, `constant` the share of
// the title's sky kept, made no brighter than that. `width` and `height` are
// the depth's.
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void miss_light(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    missed.GetDimensions(w, h);
    if (at.x >= w || at.y >= h) return;
    float2 size = float2(width, height);
    int2 p = int2((float2(at.xy) + 0.5) * size / float2(w, h));
    float3 c = seen(p, size);
    // Nothing is reflected by the sky.
    if (c.z >= 999.0) return;
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx) + float3(0, 0, -1e-9));
    if (dot(n, c) > 0.0) n = -n;
    float3 v = normalize(c);
    float3 ray = v - 2.0 * dot(v, n) * n;
    float2 g = (float2(p) + 0.5) / size * float2(PX, PY) - 0.5;
    int2 g0 = int2(floor(g));
    float2 f = g - float2(g0);
    float4 sr = 0, sg = 0, sb = 0;
    float sum = 0;
    for (int k = 0; k < 4; k++) {
        int2 q = clamp(g0 + int2(k & 1, k >> 1), int2(0, 0), int2(PX, PY) - 1);
        float z = seen(int2((float2(q) + 0.5) * size / float2(PX, PY)), size).z;
        float2 b = float2(k & 1 ? f.x : 1 - f.x, k >> 1 ? f.y : 1 - f.y);
        float wgt = (b.x * b.y + 1e-3) / (0.02 + abs(z - c.z) / c.z);
        uint at0 = (q.y * PX + q.x) * 3;
        sr += wgt * points[at0];
        sg += wgt * points[at0 + 1];
        sb += wgt * points[at0 + 2];
        sum += wgt;
    }
    float3 all = float3(sr.w, sg.w, sb.w) / sum;
    // Radiance from the first moments: three times the direction's share
    // for an even spread of directions, a little less for a smooth image.
    float3 found = all + 2.0 * float3(dot(sr.xyz, ray), dot(sg.xyz, ray), dot(sb.xyz, ray)) / sum;
    found = clamp(found, 0.1 * all, 3.0 * all);
    found = lerp(found, cells[CELLS].rgb, 0.2) * level;
    found = any(isnan(found)) || any(isinf(found)) ? 0.0 : max(found, 0.0);
    float3 was = missed[at.xy];
    was = any(isnan(was)) || any(isinf(was)) ? 0.0 : max(was, 0.0);
    // Right after a cut the estimate is of the shot before: the title's
    // light is held to its brightness, in grey, as `raise` does.
    if (cells[CUT_STATE].x > 0.0) {
        float grey = dot(found, LUMA);
        missed[at.xy] = min(was, float3(grey, grey, grey));
        return;
    }
    float has = dot(was, LUMA);
    was *= min(1.0, dot(found, LUMA) / max(has, 1e-6));
    missed[at.xy] = lerp(found, was, constant);
}
#endif

#ifdef AIR
RWTexture3D<float3> scattered : register(u0);
// No more than a multiple of the light around: of the room as the last
// seconds showed it, and of the surface seen in this direction (towards a
// window the air outside may be as bright as the sky is).
[RootSignature(RS)]
[numthreads(8, 8, 2)]
void clear_air(uint3 at : SV_DispatchThreadID) {
    uint w, h, d;
    scattered.GetDimensions(w, h, d);
    if (at.x >= w || at.y >= h || at.z >= d) return;
    uint2 q = min(uint2((float2(at.xy) + 0.5) / float2(w, h) * float2(PX, PY)), uint2(PX - 1, PY - 1));
    uint at0 = (q.y * PX + q.x) * 3;
    float3 seen_there = float3(points[at0].w, points[at0 + 1].w, points[at0 + 2].w);
    float far = float(at.z) / float(d);
    float cap = air * lerp(dot(cells[CELLS].rgb, LUMA), dot(seen_there, LUMA), far);
    float3 c = scattered[at];
    float has = dot(c, LUMA);
    if (has > cap) scattered[at] = c * (cap / has);
}
#endif

#ifdef RAISE
RWTexture2D<uint> light : register(u0);
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void raise(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    if (at.x >= width || at.y >= height) return;
    int2 p = int2(at.xy);
    float3 c = seen(p, size);
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx) + float3(0, 0, -1e-9));
    if (dot(n, c) > 0.0) n = -n;
    // How open it is (as in beloooo's cd9f78ce).
    float open = 1.0;
    float px = min(radius * size.y / (2.0 * tan_y * c.z), 128.0);
    float noise = frac(52.9829189 * frac(dot(float2(p), float2(0.06711056, 0.00583715))));
    if (strength > 0.0 && px >= 1.5 && c.z < 400.0) {
        float sum = 0.0;
        const int N = 12;
        for (int i = 0; i < N; i++) {
            float a = i * 2.399963 + noise * 6.2831853;
            float2 o = float2(cos(a), sin(a)) * (sqrt((i + 0.5) / N) * px);
            float3 v = seen(p + int2(round(o)), size) - c;
            float vv = dot(v, v);
            float over = dot(v, n) - 0.004 * c.z - 0.005;
            sum += max(over, 0.0) / sqrt(vv + 1e-6) * saturate(1.0 - vv / (radius * radius));
        }
        open = saturate(1.0 - strength * 2.0 * sum / N);
    }
    float3 found;
    if (constant > 0.0) {
        found = float3(1.0, 0.98, 0.94) * constant * open;
        light[p] = pack(found);
        return;
    }
    // Between the four points around, those at its own depth first; which
    // four is dithered a little.
    float2 g = (float2(p) + 0.5) / size * float2(PX, PY) - 0.5 + (noise - 0.5) * 0.5;
    int2 g0 = int2(floor(g));
    float2 f = g - float2(g0);
    float4 sr = 0, sg = 0, sb = 0;
    float sum = 0;
    for (int k = 0; k < 4; k++) {
        int2 q = clamp(g0 + int2(k & 1, k >> 1), int2(0, 0), int2(PX, PY) - 1);
        float z = seen(int2((float2(q) + 0.5) * size / float2(PX, PY)), size).z;
        float2 b = float2(k & 1 ? f.x : 1 - f.x, k >> 1 ? f.y : 1 - f.y);
        float w = (b.x * b.y + 1e-3) / (0.02 + abs(z - c.z) / c.z);
        uint at0 = (q.y * PX + q.x) * 3;
        sr += w * points[at0];
        sg += w * points[at0 + 1];
        sb += w * points[at0 + 2];
        sum += w;
    }
    float3 all = float3(sr.w, sg.w, sb.w) / sum;
    found = all + 1.2 * float3(dot(sr.xyz, n), dot(sg.xyz, n), dot(sb.xyz, n)) / sum;
    found = clamp(found, 0.2 * all, 2.5 * all);
    found = lerp(found, cells[CELLS].rgb, 0.2) * level * open;
    float3 was = unpack(light[p]);
    // Right after a cut the estimate is of the shot before: nothing is
    // raised to it, and what the program left (the probes' memory of that
    // shot, in blocks) is held to its brightness, in grey.
    if (cells[CUT_STATE].x > 0.0) {
        float g = dot(found, LUMA);
        light[p] = pack(min(was, float3(g, g, g)));
        return;
    }
    // What the program left counts as far as it can be true: the sky's light
    // outdoors, not the blocks of other memory.
    float top = max(dot(found, LUMA) * most, 1e-6);
    float has = dot(was, LUMA);
    // (VICTORY_AMBIENT_LEFTOVER: 2 the estimate alone, 1 the leftover's
    // brightness in the estimate's colour, 0 the larger of the two.)
    if (leftover == 2) {
        light[p] = pack(found);
        return;
    }
    if (leftover == 1) {
        light[p] = pack(found * max(1.0, min(has, top) / max(dot(found, LUMA), 1e-6)));
        return;
    }
    was *= min(1.0, top / max(has, 1e-6));
    light[p] = pack(max(was, found));
}
#endif

#ifdef HOLD
RWTexture2D<uint> reflected : register(u0);
// No brighter than a part of the light around: of the room as the last
// seconds showed it, or of the surfaces seen this way, whichever is more
// (Neptune_VI, Belobug's pass, holds them to a part of its constant light).
// Not in the frames after a cut, whose estimate is of the shot before.
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void hold(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    reflected.GetDimensions(w, h);
    if (at.x >= w || at.y >= h || cells[CUT_STATE].x > 0.0) return;
    float around;
    if (constant > 0.0) {
        around = constant;
    } else {
        uint2 q = min(uint2((float2(at.xy) + 0.5) / float2(w, h) * float2(PX, PY)), uint2(PX - 1, PY - 1));
        uint at0 = (q.y * PX + q.x) * 3;
        float3 seen_there = float3(points[at0].w, points[at0 + 1].w, points[at0 + 2].w);
        around = max(dot(cells[CELLS].rgb, LUMA), dot(seen_there, LUMA)) * level;
    }
    float top = held * around;
    float3 c = unpack(reflected[at.xy]);
    float has = dot(c, LUMA);
    if (has > top) reflected[at.xy] = pack(c * (top / max(has, 1e-9)));
}
#endif
"#;

/// Bytes of the cells' buffer and of the points'.
const CELLS: u64 = 64 * 36;
const CELL_BYTES: u64 = (3 * CELLS + 2) * 16;
const POINT_BYTES: u64 = 160 * 90 * 48;

struct Pass {
    root: ID3D12RootSignature,
    keep: ID3D12PipelineState,
    mean: ID3D12PipelineState,
    place: ID3D12PipelineState,
    gather: ID3D12PipelineState,
    raise: ID3D12PipelineState,
    air: ID3D12PipelineState,
    hold: ID3D12PipelineState,
    miss: ID3D12PipelineState,
    cells: ID3D12Resource,
    points: ID3D12Resource,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

fn make(device: &ID3D12Device, buffer: impl Fn(u64) -> Option<ID3D12Resource>) -> windows::core::Result<Pass> {
    let mut root: Option<ID3D12RootSignature> = None;
    let mut pipeline = |part: &str, entry| -> windows::core::Result<ID3D12PipelineState> {
        let code = crate::host::compile(&format!("#define {part}\n{SOURCE}"), entry, s!("cs_5_1"))?;
        // SAFETY: the root signature is inside the shader; plain creations.
        unsafe {
            if root.is_none() {
                root = Some(device.CreateRootSignature(0, &code)?);
            }
            let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                ..Default::default()
            };
            device.CreateComputePipelineState(&desc)
        }
    };
    let keep = pipeline("KEEP", s!("keep"))?;
    let mean = pipeline("KEEP", s!("mean"))?;
    let place = pipeline("PLACE", s!("place"))?;
    let gather = pipeline("GATHER", s!("gather"))?;
    let raise = pipeline("RAISE", s!("raise"))?;
    let air = pipeline("AIR", s!("clear_air"))?;
    let hold = pipeline("HOLD", s!("hold"))?;
    let miss = pipeline("MISS", s!("miss_light"))?;
    let made = |bytes| buffer(bytes).ok_or_else(windows::core::Error::empty);
    Ok(Pass {
        root: root.ok_or_else(windows::core::Error::empty)?,
        keep,
        mean,
        place,
        gather,
        raise,
        air,
        hold,
        miss,
        cells: made(CELL_BYTES)?,
        points: made(POINT_BYTES)?,
    })
}

fn pass() -> Option<&'static Pass> {
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    PASS.get_or_init(|| {
        let host = host()?;
        make(&host.device, |bytes| host.uav_buffer(bytes))
            .inspect_err(|e| tracing::warn!(error = %e, "no stand-in for the indirect light"))
            .ok()
    })
    .as_ref()
}

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// Whether the light of a frame has been kept: the estimate has something
/// to begin with.
static KEPT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the passes read and write, as views in the heap the list has set.
pub(crate) struct Views {
    /// The image the title's program just wrote.
    pub(crate) image: D3D12_GPU_DESCRIPTOR_HANDLE,
    /// The scene's depth, for [`Role::Resolve`] and [`Role::Miss`].
    pub(crate) depth: Option<D3D12_GPU_DESCRIPTOR_HANDLE>,
    /// The size of the image, and how many slices it has.
    pub(crate) size: [u32; 2],
    pub(crate) slices: u32,
    /// The size of the depth (the image's, but for [`Role::Miss`]: the
    /// picture's, twice the image each way).
    pub(crate) depth_size: [u32; 2],
}

/// For the line every ten seconds: what the stand-in for reflections did.
struct Said {
    /// Dispatches of the miss program it followed, frames they were in,
    /// and those it left as they were: of water or the alpha layer, and
    /// before a frame's light was kept (or with no tables found).
    ran: AtomicU64,
    frames: AtomicU64,
    water: AtomicU64,
    not_ready: AtomicU64,
    last_frame: AtomicU64,
    /// Bit 0 opaque surfaces, 1 alpha, 2 water, 3 others.
    layers: AtomicU64,
}

static SAID_MISS: Said = Said {
    ran: AtomicU64::new(0),
    frames: AtomicU64::new(0),
    water: AtomicU64::new(0),
    not_ready: AtomicU64::new(0),
    last_frame: AtomicU64::new(u64::MAX),
    layers: AtomicU64::new(0),
};

/// What came of a dispatch of the miss program, counted for the line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum MissNote {
    /// Followed by the stand-in; the layer its rays are of.
    Ran(u32),
    /// Of water or of the alpha layer: left as the title has it.
    Water,
    /// No light kept yet, or its image or depth not found.
    NotReady,
}

/// Counts what came of a dispatch of the miss program; every ten seconds an
/// info line says it, so a run shows that the stand-in ran and where.
pub(crate) fn note_miss(note: MissNote) {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(Specular { share, sky }) = specular() else {
        return;
    };
    let s = &SAID_MISS;
    match note {
        MissNote::Ran(layer) => {
            s.ran.fetch_add(1, Relaxed);
            s.layers.fetch_or(1 << layer.min(3), Relaxed);
            let frame = crate::frame::current();
            if s.last_frame.swap(frame, Relaxed) != frame {
                s.frames.fetch_add(1, Relaxed);
            }
        }
        MissNote::Water => {
            s.water.fetch_add(1, Relaxed);
        }
        MissNote::NotReady => {
            s.not_ready.fetch_add(1, Relaxed);
        }
    }
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let Some(since) = *last else {
        *last = Some(now);
        return;
    };
    if now.duration_since(since).as_secs_f32() < 10.0 {
        return;
    }
    *last = Some(now);
    let layers = s.layers.swap(0, Relaxed);
    let named: Vec<&str> = [(1, "opaque"), (2, "alpha"), (4, "water"), (8, "other")]
        .iter()
        .filter(|(bit, _)| layers & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    tracing::info!(
        "a stand-in for reflections (VICTORY_AMBIENT_SPECULAR={share},{sky}) in the last {:.0} s: \
         {} frames, {} dispatches of CS_RayMiss followed (surfaces: {}), each by an estimate along \
         the reflected direction put in right after it (before CS_ResolveScreenSpace); {} of water or the alpha layer \
         and {} before a frame's light was kept, or without their image and depth found, left as the title had them",
        now.duration_since(since).as_secs_f32(),
        s.frames.swap(0, Relaxed),
        s.ran.swap(0, Relaxed),
        if named.is_empty() { "none".to_owned() } else { named.join(", ") },
        s.water.swap(0, Relaxed),
        s.not_ready.swap(0, Relaxed),
    );
}

/// The frame in which the stand-in for reflections last placed the cells
/// (`place`, `gather`): another placing in that frame, its own or the
/// resolve's, leaves where the cells were the frame before and does not
/// look for a camera cut again (a negative `cut` for the passes). Else the
/// resolve, which comes later in the frame, would compare the frame with
/// itself and never see a cut, and the cut's window would run out twice as
/// fast.
static MISS_PLACED: AtomicU64 = AtomicU64::new(u64::MAX);

/// After the title's program of that role, in `list` with the program's view
/// heap set. Replaces the list's compute root signature and pipeline; false
/// if it recorded nothing.
pub(crate) fn after(list: &HostList, role: Role, views: &Views) -> bool {
    use std::sync::atomic::Ordering;
    let Some(pass) = pass() else {
        return false;
    };
    // The share of the estimate, and the constant (for reflections: the
    // share of the title's sky kept); the light is kept for either stand-in.
    let (level, constant) = match (role, set(), specular()) {
        (Role::Miss, _, Some(s)) => (s.share, s.sky),
        (Role::Miss, _, None) => return false,
        (Role::Radiance, Set::Off, Some(_)) => (0.0, 0.0),
        (_, Set::Off, _) => return false,
        (_, Set::Estimate(level), _) => (level, 0.0f32),
        (_, Set::Constant(level), _) => (0.0, level),
    };
    let estimate = role == Role::Miss || constant == 0.0;
    if role != Role::Radiance && estimate && !KEPT.load(Ordering::Relaxed) {
        if role == Role::Miss {
            note_miss(MissNote::NotReady);
        }
        return false;
    }
    if role == Role::Air && (constant > 0.0 || air() == 0.0) {
        return false;
    }
    if role == Role::Reflect && reflect() == 0.0 {
        return false;
    }
    static SAID: Mutex<u32> = Mutex::new(0);
    {
        let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
        if *said < 5 {
            *said += 1;
            tracing::info!(?role, size = ?views.size, level, constant, "a stand-in for the indirect light");
        }
    }
    list.keep(&pass.cells);
    list.keep(&pass.points);
    let [strength, radius] = shade();
    let floats = [level, constant, strength, radius, 0.1, 0.45];
    let mut words = [0u32; 14];
    for (to, v) in words.iter_mut().zip(floats) {
        *to = v.to_bits();
    }
    words[6] = views.depth_size[0];
    words[7] = views.depth_size[1];
    words[8] = 0.04f32.to_bits();
    words[9] = 4.0f32.to_bits();
    words[10] = air().to_bits();
    let frame = crate::frame::current();
    let placed_already = match role {
        Role::Miss => MISS_PLACED.swap(frame, Ordering::Relaxed) == frame,
        Role::Resolve => MISS_PLACED.load(Ordering::Relaxed) == frame,
        _ => false,
    };
    words[11] = if placed_already { -1.0f32 } else { cut() }.to_bits();
    words[12] = reflect().to_bits();
    words[13] = leftover();
    let l = &list.list;
    // SAFETY: an open list with the heap of the views set; the title's
    // program has just used both views, so their textures are in the states
    // these views need; buffers the list keeps.
    unsafe {
        uav_barrier(l);
        l.SetComputeRootSignature(&pass.root);
        l.SetComputeRoot32BitConstants(0, 14, words.as_ptr().cast(), 0);
        if let Some(depth) = views.depth {
            l.SetComputeRootDescriptorTable(1, depth);
        }
        l.SetComputeRootDescriptorTable(2, views.image);
        l.SetComputeRootUnorderedAccessView(3, pass.cells.GetGPUVirtualAddress());
        l.SetComputeRootUnorderedAccessView(4, pass.points.GetGPUVirtualAddress());
        let run = |pso: &ID3D12PipelineState, x, y, z| {
            l.SetPipelineState(pso);
            l.Dispatch(x, y, z);
            uav_barrier(l);
        };
        match role {
            Role::Radiance => {
                run(&pass.keep, 8, 9, 1);
                run(&pass.mean, 1, 1, 1);
                KEPT.store(true, Ordering::Relaxed);
            }
            Role::Air => run(&pass.air, views.size[0].div_ceil(8), views.size[1].div_ceil(8), views.slices.div_ceil(2)),
            Role::Reflect => run(&pass.hold, views.size[0].div_ceil(8), views.size[1].div_ceil(8), 1),
            Role::Resolve => {
                if constant == 0.0 {
                    run(&pass.place, 8, 9, 1);
                    run(&pass.gather, 20, 45, 1);
                }
                run(&pass.raise, views.size[0].div_ceil(8), views.size[1].div_ceil(8), 1);
            }
            Role::Miss => {
                // The points afresh, from this frame's depth: the diffuse
                // stand-in makes them later in the frame.
                run(&pass.place, 8, 9, 1);
                run(&pass.gather, 20, 45, 1);
                run(&pass.miss, views.size[0].div_ceil(8), views.size[1].div_ceil(8), 1);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        assert_eq!(parse(""), Set::Off);
        assert_eq!(parse("off"), Set::Off);
        assert_eq!(parse("0"), Set::Off);
        assert_eq!(parse("0.8"), Set::Estimate(0.8));
        assert_eq!(parse("constant:0.3"), Set::Constant(0.3));
        assert_eq!(parse("constant:nan"), Set::Off);
    }

    #[test]
    fn specular_settings() {
        assert_eq!(parse_specular(""), None);
        assert_eq!(parse_specular("0"), None);
        assert_eq!(parse_specular("off"), None);
        assert_eq!(parse_specular("-1"), None);
        assert_eq!(parse_specular("1"), Some(Specular { share: 1.0, sky: 0.0 }));
        assert_eq!(parse_specular("0.6, 0.25"), Some(Specular { share: 0.6, sky: 0.25 }));
        assert_eq!(parse_specular("0.6,7"), Some(Specular { share: 0.6, sky: 1.0 }));
        assert_eq!(parse_specular("0.6,x"), Some(Specular { share: 0.6, sky: 0.0 }));
    }

    #[test]
    fn cut_window() {
        assert_eq!(cut_frames(None), 2.0);
        assert_eq!(cut_frames(Some("")), 2.0);
        assert_eq!(cut_frames(Some("0")), 0.0);
        assert_eq!(cut_frames(Some("3")), 3.0);
        assert_eq!(cut_frames(Some("2.7")), 2.0);
        assert_eq!(cut_frames(Some("100")), 30.0);
        assert_eq!(cut_frames(Some("-1")), 2.0);
        assert_eq!(cut_frames(Some("x")), 2.0);
    }

    #[test]
    fn roles_by_name() {
        assert_eq!(role(b"..CS_ScreenProbes_TemporalResolve_a911b49c_Wrapped.."), Some(Role::Resolve));
        assert_eq!(role(b"..CS_CacheScreenSpaceRadiance_eed2fc35.."), Some(Role::Radiance));
        assert_eq!(role(b"CS_AccumulateLocalLightInscatteringVolume_7e2f5eef"), Some(Role::Air));
        assert_eq!(role(b"CS_TemporalResolve"), None);
        assert_eq!(role(b"..CS_RayMiss_de6b91a6_Wrapped.."), Some(Role::Miss));
        assert_eq!(role(b"..CS_RayMiss_Volumetrics_3c5d8c00_Wrapped.."), None);
        assert_eq!(role(b"..CS_ScreenProbe_RayMiss_80a88544_Wrapped.."), None);
        assert_eq!(role(b"..CS_RayMiss_de6b91a6"), None);
        assert_eq!(role(b"..CS_UpdateHistory_1f2e3d4c.."), Some(Role::Reflect));
    }

    #[test]
    fn reflections_held() {
        assert_eq!(reflect_part(None), 0.0);
        assert_eq!(reflect_part(Some("")), 0.0);
        assert_eq!(reflect_part(Some("0")), 0.0);
        assert_eq!(reflect_part(Some("0.5")), 0.5);
        assert_eq!(reflect_part(Some(" 2 ")), 2.0);
        assert_eq!(reflect_part(Some("100")), 16.0);
        assert_eq!(reflect_part(Some("-1")), 0.0);
        assert_eq!(reflect_part(Some("nan")), 0.0);
        assert_eq!(reflect_part(Some("x")), 0.0);
    }

    /// `VICTORY_AMBIENT_LEFTOVER`: today's `keep` unless `hue` or `off`.
    #[test]
    fn leftover_settings() {
        assert_eq!(leftover_mode(""), 0);
        assert_eq!(leftover_mode("keep"), 0);
        assert_eq!(leftover_mode(" Hue "), 1);
        assert_eq!(leftover_mode("off"), 2);
        assert_eq!(leftover_mode("0"), 2);
        assert_eq!(leftover_mode("whatever"), 0);
    }

    /// The passes compile with the system's compiler and the host takes them.
    #[test]
    fn the_passes_build() {
        let Some(host) = host() else {
            return;
        };
        make(&host.device, |bytes| host.uav_buffer(bytes)).expect("the passes");
    }
}
