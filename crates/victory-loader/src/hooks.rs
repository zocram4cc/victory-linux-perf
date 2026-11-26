//! Opt-in compatibility hooks for known title builds (`VICTORY_NO_WATCHDOG`,
//! `VICTORY_NO_POOL_TRAP`, `VICTORY_RT_FALLBACK`,
//! `VICTORY_RT_FALLBACK_NO_BVH`; each off without it, or set to "0").
//!
//! Only per-build offsets and short byte signatures live here (asset policy:
//! nothing of the title itself). Every hook checks the build's hash and the
//! exact bytes it replaces ([`resolution::write`]), and leaves a place whose
//! bytes differ alone, said so.

use crate::frame_rate::Patched;
use crate::resolution::{self, Site};

/// First 16 hex digits of the BLAKE3 of the Series X 1.0.2.1 executable.
const BUILD_1_0_2_1: &str = "b99f876b74d4a77f";

/// One set of patches and the environment variable that turns it on.
pub(crate) struct Hook {
    pub(crate) env: &'static str,
    patches: &'static [Patch],
}

/// The opt-in hooks, each applied when its variable is set (not "0").
pub(crate) const HOOKS: &[Hook] = &[
    Hook {
        env: "VICTORY_NO_WATCHDOG",
        patches: WATCHDOG,
    },
    Hook {
        env: "VICTORY_NO_POOL_TRAP",
        patches: POOL_TRAP,
    },
    Hook {
        env: "VICTORY_RT_FALLBACK",
        patches: RT_FALLBACK,
    },
    Hook {
        env: "VICTORY_RT_FALLBACK_NO_BVH",
        patches: RT_FALLBACK_NO_BVH,
    },
];

/// One code patch: the site's bytes, then what replaces them (same length).
struct Patch {
    site: Site,
    new: &'static [u8],
}

/// The title's watchdog thread ends the process on purpose when its main
/// thread has not advanced for about 30 one-second ticks. Two places do it:
/// the handler it calls (`mov dword ptr [0], imm32`) and, when the handler
/// returns or is unset, a fallback right after the call (`mov dword ptr [0],
/// 0; int3`). Jumping over the whole block returns the loop to its normal
/// path; the second patch makes the handler harmless if something else calls it.
const WATCHDOG: &[Patch] = &[
    Patch {
        site: Site {
            at: 0x2215c27,
            what: "watchdog: skip the call and the fallback fault",
            // mov rax, [rip + disp32] (the handler pointer)
            bytes: &[0x48, 0x8B, 0x05, 0x1A, 0x07, 0xFA, 0x04],
        },
        // jmp +0x18 (to the loop's normal continuation), then NOPs
        new: &[0xEB, 0x18, 0x90, 0x90, 0x90, 0x90, 0x90],
    },
    Patch {
        site: Site {
            at: 0x23cf683,
            what: "watchdog: handler's deliberate fault",
            bytes: &[0xC7, 0x04, 0x25, 0, 0, 0, 0, 0x21, 0xD4, 0x8B, 0x4D],
        },
        // an eleven-byte NOP
        new: &[0x66, 0x66, 0x2E, 0x0F, 0x1F, 0x84, 0, 0, 0, 0, 0],
    },
];

/// About six minutes into the prologue an allocator of the title's instance
/// lists finds its pool full (upstream issue #698) and stops on an `int3`
/// placed before its NULL return. Going on returns NULL to the caller, which
/// copes: what needed the list is not drawn, the game goes on (slauko's runs
/// in #698: 30 minutes, no fault). A way past the stop, not its cause.
const POOL_TRAP: &[Patch] = &[Patch {
    site: Site {
        at: 0x5149aea,
        what: "instance-list pool: return NULL instead of the trap",
        // jmp +3 (the allocated path, over the trap); int3; xor edi, edi
        bytes: &[0xEB, 0x03, 0xCC, 0x31, 0xFF],
    },
    // the int3 becomes a nop: the NULL path goes on to return NULL
    new: &[0xEB, 0x03, 0x90, 0x31, 0xFF],
}];

/// The title's ray-traced lighting binds its top-level acceleration structure
/// only where +0x4fd8550 says slot 0 holds one ("is the TLAS there?"); all
/// four binders (screen probes +0x356b6b0, +0x35744e0, +0x35797d0,
/// +0x35915a0) take a fallback branch of the title's own when it says no
/// (e.g. `ScreenProbes_TraceRays_Fallback`). The patch makes it always say
/// no: the title's non-ray-traced lighting, with the structures still built
/// (the voxel passes in the shader pack are never selected on this build).
const NO_TLAS: Patch = Patch {
    site: Site {
        at: 0x4fd8550,
        what: "ray tracing: the top-level structure reads as absent (the title's fallback lighting)",
        // mov edx, ecx; mov rcx, [rip + disp32]; test rcx, rcx
        bytes: &[
            0x89, 0xCA, 0x48, 0x8B, 0x0D, 0x6F, 0xF8, 0x54, 0x03, 0x48, 0x85, 0xC9,
        ],
    },
    // xor eax, eax; ret (the rest stays, unreached)
    new: &[
        0x31, 0xC0, 0xC3, 0x8B, 0x0D, 0x6F, 0xF8, 0x54, 0x03, 0x48, 0x85, 0xC9,
    ],
};

/// [`NO_TLAS`] alone: the title's fallback lighting.
const RT_FALLBACK: &[Patch] = &[NO_TLAS];

/// [`NO_TLAS`] plus no work for `XboxBvhBuilder.dll`: its build, rehydrate
/// and copy calls each return early for a count of zero; the six places
/// that call them (two command-list kinds each) take that early return
/// always (`test reg, reg` becomes `xor reg, reg`). Never without
/// [`NO_TLAS`]: shaders would then trace structures that were never built.
const RT_FALLBACK_NO_BVH: &[Patch] = &[
    NO_TLAS,
    Patch {
        site: Site {
            at: 0xfe4e7d,
            what: "BVH builds skipped (lists A)",
            bytes: &[0x85, 0xF6, 0x0F, 0x84, 0xFB, 0x06, 0x00, 0x00],
        },
        new: &[0x31, 0xF6, 0x0F, 0x84, 0xFB, 0x06, 0x00, 0x00],
    },
    Patch {
        site: Site {
            at: 0x249013d,
            what: "BVH builds skipped (lists B)",
            bytes: &[0x85, 0xF6, 0x0F, 0x84, 0xFB, 0x06, 0x00, 0x00],
        },
        new: &[0x31, 0xF6, 0x0F, 0x84, 0xFB, 0x06, 0x00, 0x00],
    },
    Patch {
        site: Site {
            at: 0xea6c7f,
            what: "BVH rehydration skipped (lists A)",
            bytes: &[0x85, 0xFF, 0x0F, 0x84, 0x40, 0x02, 0x00, 0x00],
        },
        new: &[0x31, 0xFF, 0x0F, 0x84, 0x40, 0x02, 0x00, 0x00],
    },
    Patch {
        site: Site {
            at: 0x2490a3f,
            what: "BVH rehydration skipped (lists B)",
            bytes: &[0x85, 0xFF, 0x0F, 0x84, 0x40, 0x02, 0x00, 0x00],
        },
        new: &[0x31, 0xFF, 0x0F, 0x84, 0x40, 0x02, 0x00, 0x00],
    },
    Patch {
        site: Site {
            at: 0xbd96b0,
            what: "BVH copies skipped (lists A)",
            bytes: &[0x45, 0x85, 0xC9, 0x0F, 0x84, 0xF4, 0x00, 0x00, 0x00],
        },
        new: &[0x45, 0x31, 0xC9, 0x0F, 0x84, 0xF4, 0x00, 0x00, 0x00],
    },
    Patch {
        site: Site {
            at: 0x2490900,
            what: "BVH copies skipped (lists B)",
            bytes: &[0x45, 0x85, 0xC9, 0x0F, 0x84, 0xF4, 0x00, 0x00, 0x00],
        },
        new: &[0x45, 0x31, 0xC9, 0x0F, 0x84, 0xF4, 0x00, 0x00, 0x00],
    },
];

/// Applies the hooks whose variables are set to the image of `size` bytes
/// at `base`, whose original BLAKE3 is `hash`. A line each in the log.
///
/// # Safety
/// The image stays mapped and no thread runs its code yet.
pub(crate) unsafe fn apply_asked(base: usize, size: usize, hash: &blake3::Hash) {
    for hook in HOOKS {
        if !std::env::var_os(hook.env).is_some_and(|v| !v.is_empty() && v != "0") {
            continue;
        }
        if !hash.to_hex().starts_with(BUILD_1_0_2_1) {
            tracing::warn!(
                hook = hook.env,
                "title hook not applied: not the build it was made for (1.0.2.1)"
            );
            continue;
        }
        let mut applied = 0;
        for p in hook.patches {
            // SAFETY: the caller's promise.
            let done = unsafe { resolution::write(base, size, &p.site, p.new) };
            let site = format!("{:#x}", p.site.at);
            match done {
                Patched::Done => applied += 1,
                Patched::Differs => tracing::warn!(
                    site,
                    what = p.site.what,
                    "title hook: not applied, the title's bytes there are not the ones expected"
                ),
                Patched::Failed(why) => {
                    tracing::warn!(site, what = p.site.what, why, "title hook: not applied")
                }
            }
        }
        tracing::info!(
            applied,
            of = hook.patches.len(),
            "title hooks ({})",
            hook.env
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every replacement is as long as the bytes it replaces.
    #[test]
    fn replacements_fit() {
        for hook in HOOKS {
            for p in hook.patches {
                assert_eq!(p.new.len(), p.site.bytes.len(), "{}", p.site.what);
            }
        }
    }
}
