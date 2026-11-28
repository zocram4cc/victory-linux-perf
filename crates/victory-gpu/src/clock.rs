use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Instant(u64);

struct Scale {
    base: u64,
    mult: u64,
}

fn scale() -> &'static Option<Scale> {
    static SCALE: OnceLock<Option<Scale>> = OnceLock::new();
    SCALE.get_or_init(|| {
        let began = std::time::Instant::now();
        #[cfg(target_arch = "x86_64")]
        {
            let invariant = std::arch::x86_64::__cpuid(0x8000_0000).eax >= 0x8000_0007 && std::arch::x86_64::__cpuid(0x8000_0007).edx & 1 << 8 != 0;
            if !invariant || std::env::var_os("VICTORY_CLOCK").is_some_and(|v| v == "system") {
                return None;
            }
            let read = || {
                let before = unsafe { std::arch::x86_64::_rdtsc() };
                let now = std::time::Instant::now();
                let after = unsafe { std::arch::x86_64::_rdtsc() };
                (before / 2 + after / 2, now)
            };
            let (ticks, at) = read();
            std::thread::sleep(Duration::from_millis(12));
            let (later, then) = read();
            let (ticked, nanos) = (later.wrapping_sub(ticks), then.duration_since(at).as_nanos() as u64);
            if ticked < 1_000_000 || nanos == 0 {
                return None;
            }
            let mult = ((u128::from(nanos) << 32) / u128::from(ticked)) as u64;
            let base = ticks.wrapping_sub(((u128::from(at.duration_since(began).as_nanos() as u64) << 32) / u128::from(mult.max(1))) as u64);
            Some(Scale { base, mult })
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = began;
            None
        }
    })
}

fn system() -> &'static std::time::Instant {
    static BEGAN: OnceLock<std::time::Instant> = OnceLock::new();
    BEGAN.get_or_init(std::time::Instant::now)
}

impl Instant {
    #[inline]
    pub(crate) fn now() -> Instant {
        match scale() {
            #[cfg(target_arch = "x86_64")]
            Some(s) => {
                let ticks = unsafe { std::arch::x86_64::_rdtsc() }.wrapping_sub(s.base);
                Instant(((u128::from(ticks) * u128::from(s.mult)) >> 32) as u64)
            }
            #[cfg(not(target_arch = "x86_64"))]
            Some(_) => Instant(system().elapsed().as_nanos() as u64),
            None => Instant(system().elapsed().as_nanos() as u64),
        }
    }

    #[inline]
    pub(crate) fn elapsed(&self) -> Duration {
        Instant::now().duration_since(*self)
    }

    #[inline]
    pub(crate) fn duration_since(&self, earlier: Instant) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }

    #[inline]
    pub(crate) fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.duration_since(earlier)
    }
}

impl std::ops::Add<Duration> for Instant {
    type Output = Instant;
    #[inline]
    fn add(self, more: Duration) -> Instant {
        Instant(self.0.saturating_add(more.as_nanos() as u64))
    }
}

impl std::ops::AddAssign<Duration> for Instant {
    #[inline]
    fn add_assign(&mut self, more: Duration) {
        *self = *self + more;
    }
}

impl std::ops::Sub<Duration> for Instant {
    type Output = Instant;
    #[inline]
    fn sub(self, less: Duration) -> Instant {
        Instant(self.0.saturating_sub(less.as_nanos() as u64))
    }
}

impl std::ops::Sub<Instant> for Instant {
    type Output = Duration;
    #[inline]
    fn sub(self, earlier: Instant) -> Duration {
        self.duration_since(earlier)
    }
}
