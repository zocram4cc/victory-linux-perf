//! Repeated reads of ordinary sampled textures by one translation worker.
use super::{BUFFER, This, host_copy, resource};
use crate::host::HostList;
use std::sync::atomic::Ordering::Relaxed;

/// Called for a direct shader SRV that isn't the tested depth target.
/// Only an ordinary texture (no buffer range, alias or depth twin) can
/// take this path. Placed views can change their first mip or host copy.
pub(crate) fn prepare(list: &HostList, this: This) -> bool {
    if this.is_null() {
        return false;
    }
    let r = resource(this);
    const TARGET_DEPTH_OR_UAV: u32 = 0x7;
    if r.desc.dimension == BUFFER
        || r.desc.flags & TARGET_DEPTH_OR_UAV != 0
        || r.host.placed.load(Relaxed)
    {
        return false;
    }
    let epoch = list.epoch();
    if r.host.read_epoch.load(Relaxed) == epoch {
        // The list's states hold this copy through reset. Keep the
        // last-use time that `hosted` would refresh under its mutex.
        r.host.touch();
        return true;
    }
    let Some(copy) = host_copy(this) else {
        return false;
    };
    if copy.flags() & TARGET_DEPTH_OR_UAV != 0 {
        return false;
    }
    crate::texture::upload_from(list, this, 0);
    crate::list::ensure_state(list, this, crate::draw::SHADER_RESOURCE);
    // A state's first insertion and its changes advance the epoch. Save
    // the resulting epoch. Reset and other lists have unique epochs. Two
    // readers on different workers can only evict one another's stamp.
    r.host.read_epoch.store(list.epoch(), Relaxed);
    true
}
