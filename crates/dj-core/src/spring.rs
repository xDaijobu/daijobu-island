//! Damped spring, mass 1: `omega = 2*pi/response`, `k = omega^2`, `c = 2*zeta*omega`.
//! Semi-implicit Euler in fixed 1/240 s substeps. Retargeting keeps velocity.

use std::f32::consts::TAU;

pub const SUBSTEP: f32 = 1.0 / 240.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSpec {
    pub response: f32,
    pub damping: f32,
}

impl SpringSpec {
    pub const fn new(response: f32, damping: f32) -> Self {
        Self { response, damping }
    }

    pub fn stiffness(self) -> f32 {
        let w = TAU / self.response;
        w * w
    }

    pub fn drag(self) -> f32 {
        2.0 * self.damping * (TAU / self.response)
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self::new(
            self.response + (other.response - self.response) * t,
            self.damping + (other.damping - self.damping) * t,
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub spec: SpringSpec,
    eps: f32,
    leftover: f32,
}

impl Spring {
    /// `eps` is the distance (in the spring's own units) under which it counts as at rest.
    pub fn new(value: f32, spec: SpringSpec, eps: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            spec,
            eps,
            leftover: 0.0,
        }
    }

    /// Moves the target; the current velocity is preserved.
    pub fn retarget(&mut self, target: f32, spec: SpringSpec) {
        self.target = target;
        self.spec = spec;
    }

    pub fn step(&mut self, dt: f32) {
        if self.settled() {
            return;
        }
        let (k, c) = (self.spec.stiffness(), self.spec.drag());
        self.leftover += dt.max(0.0);
        while self.leftover >= SUBSTEP {
            self.leftover -= SUBSTEP;
            let accel = -k * (self.value - self.target) - c * self.velocity;
            self.velocity += accel * SUBSTEP;
            self.value += self.velocity * SUBSTEP;
        }
        if self.settled() {
            self.value = self.target;
            self.velocity = 0.0;
            self.leftover = 0.0;
        }
    }

    pub fn settled(&self) -> bool {
        (self.value - self.target).abs() < self.eps && self.velocity.abs() < self.eps * 10.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Spring, secs: f32) -> f32 {
        let mut peak = s.value;
        let mut t = 0.0;
        while t < secs {
            s.step(1.0 / 144.0);
            peak = peak.max(s.value);
            t += 1.0 / 144.0;
        }
        peak
    }

    #[test]
    fn constants_match_plan_table() {
        let e = SpringSpec::new(0.45, 0.75);
        assert!((e.stiffness() - 195.0).abs() < 1.0);
        assert!((e.drag() - 21.0).abs() < 0.5);
        let c = SpringSpec::new(0.35, 0.95);
        assert!((c.stiffness() - 322.0).abs() < 1.5);
        assert!((c.drag() - 34.0).abs() < 0.5);
        let f = SpringSpec::new(0.25, 1.0);
        assert!((f.stiffness() - 632.0).abs() < 1.5);
        assert!((f.drag() - 50.0).abs() < 0.5);
    }

    #[test]
    fn underdamped_overshoots_then_settles() {
        let mut s = Spring::new(0.0, SpringSpec::new(0.45, 0.75), 0.01);
        s.retarget(100.0, SpringSpec::new(0.45, 0.75));
        let peak = run(&mut s, 3.0);
        assert!(peak > 102.0, "expected overshoot, peak {peak}");
        assert!(s.settled());
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn critical_damping_does_not_overshoot() {
        let mut s = Spring::new(0.0, SpringSpec::new(0.25, 1.0), 0.001);
        s.retarget(1.0, SpringSpec::new(0.25, 1.0));
        assert!(run(&mut s, 2.0) <= 1.0 + 1e-4);
        assert!(s.settled());
    }

    #[test]
    fn retarget_keeps_velocity() {
        let spec = SpringSpec::new(0.45, 0.75);
        let mut s = Spring::new(0.0, spec, 0.01);
        s.retarget(100.0, spec);
        for _ in 0..10 {
            s.step(1.0 / 144.0);
        }
        let v = s.velocity;
        assert!(v > 0.0);
        s.retarget(0.0, SpringSpec::new(0.35, 0.95));
        assert_eq!(s.velocity, v);
        assert!(!s.settled());
    }

    #[test]
    fn idle_spring_is_settled_and_stays_put() {
        let mut s = Spring::new(5.0, SpringSpec::new(0.45, 0.75), 0.01);
        assert!(s.settled());
        s.step(1.0);
        assert_eq!(s.value, 5.0);
    }
}
