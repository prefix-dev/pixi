#![allow(non_camel_case_types)]

use pyo3::prelude::*;
use std::os::raw::{c_uint, c_void};

// ===========================================================================
// C ABI mirror of particle_core/include/particle_core/particle.h
// ===========================================================================

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct pc_vec2_t {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct pc_particle_t {
    pub position: pc_vec2_t,
    pub velocity: pc_vec2_t,
    pub age: f32,
    pub lifetime: f32,
    pub size: f32,
    pub color: u32,
}

// ===========================================================================
// xorshift32 RNG
// ===========================================================================

fn xorshift32(s: &mut u32) -> u32 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = if x == 0 { 1 } else { x };
    *s
}

fn frand_unit(s: &mut u32) -> f32 {
    xorshift32(s) as f32 / u32::MAX as f32
}

// ===========================================================================
// Ring emitter
// ===========================================================================

#[pyclass]
struct Ring {
    #[pyo3(get, set)] x:        f32,
    #[pyo3(get, set)] y:        f32,
    #[pyo3(get, set)] radius:   f32,
    #[pyo3(get, set)] speed:    f32,
    #[pyo3(get, set)] rate:     f32,
    #[pyo3(get, set)] lifetime: f32,
    #[pyo3(get, set)] size:     f32,
    #[pyo3(get, set)] color:    u32,
    #[pyo3(get, set)] rng:      u32,
    accumulator: f32,
}

impl Ring {
    fn emit(&mut self, out: *mut pc_particle_t, max: c_uint, dt: f32) -> c_uint {
        self.accumulator += self.rate * dt;
        let to_emit = (self.accumulator as u32).min(max);
        self.accumulator -= to_emit as f32;
        if to_emit == 0 {
            return 0;
        }
        let out_slice = unsafe { std::slice::from_raw_parts_mut(out, to_emit as usize) };
        for p in out_slice.iter_mut() {
            let theta = frand_unit(&mut self.rng) * std::f32::consts::TAU;
            let (sn, cs) = theta.sin_cos();
            p.position = pc_vec2_t {
                x: self.x + cs * self.radius,
                y: self.y + sn * self.radius,
            };
            p.velocity = pc_vec2_t {
                x: -sn * self.speed,
                y:  cs * self.speed,
            };
            p.age = 0.0;
            p.lifetime = self.lifetime;
            p.size = self.size;
            p.color = self.color;
        }
        to_emit
    }
}

extern "C" fn ring_emit_thunk(data: *mut c_void, out: *mut pc_particle_t, max: c_uint, dt: f32) -> c_uint {
    let ring = unsafe { &mut *(data as *mut Ring) };
    ring.emit(out, max, dt)
}

#[pymethods]
impl Ring {
    #[new]
    #[pyo3(signature = (
        x        = 400.0,
        y        = 300.0,
        radius   = 80.0,
        speed    = 80.0,
        rate     = 60.0,
        lifetime = 2.0,
        size     = 3.0,
        color    = 0x44CCFFFF,
        rng      = 0xA5A5_A5A5,
    ))]
    fn new(x: f32, y: f32, radius: f32, speed: f32, rate: f32,
           lifetime: f32, size: f32, color: u32, rng: u32) -> Self {
        Ring { x, y, radius, speed, rate, lifetime, size, color, rng, accumulator: 0.0 }
    }

    fn c_interface(slf: PyRef<'_, Self>) -> (usize, usize, usize) {
        (
            &*slf as *const Self as usize,
            ring_emit_thunk as usize,
            0,
        )
    }
}

// ===========================================================================
// Vortex modifier
// ===========================================================================

#[pyclass]
struct Vortex {
    #[pyo3(get, set)] x:        f32,
    #[pyo3(get, set)] y:        f32,
    #[pyo3(get, set)] strength: f32,
    #[pyo3(get, set)] falloff:  f32,
}

impl Vortex {
    fn apply(&self, p: *mut pc_particle_t, count: c_uint, dt: f32) {
        if count == 0 {
            return;
        }
        let particles = unsafe { std::slice::from_raw_parts_mut(p, count as usize) };
        let f2 = self.falloff * self.falloff;
        for q in particles.iter_mut() {
            let dx = q.position.x - self.x;
            let dy = q.position.y - self.y;
            let r2 = dx * dx + dy * dy;
            let scale = f2 / (r2 + f2);
            q.velocity.x += -dy * self.strength * scale * dt;
            q.velocity.y +=  dx * self.strength * scale * dt;
        }
    }
}

extern "C" fn vortex_apply_thunk(data: *mut c_void, p: *mut pc_particle_t, count: c_uint, dt: f32) {
    let vortex = unsafe { &*(data as *const Vortex) };
    vortex.apply(p, count, dt)
}

#[pymethods]
impl Vortex {
    #[new]
    #[pyo3(signature = (x = 400.0, y = 300.0, strength = 1.5, falloff = 200.0))]
    fn new(x: f32, y: f32, strength: f32, falloff: f32) -> Self {
        Vortex { x, y, strength, falloff }
    }

    fn c_interface(slf: PyRef<'_, Self>) -> (usize, usize, usize) {
        (
            &*slf as *const Self as usize,
            vortex_apply_thunk as usize,
            0,
        )
    }
}

// ===========================================================================
// Attractor modifier
// ===========================================================================

#[pyclass]
struct Attractor {
    #[pyo3(get, set)] x:         f32,
    #[pyo3(get, set)] y:         f32,
    #[pyo3(get, set)] strength:  f32,
    #[pyo3(get, set)] softening: f32,
}

impl Attractor {
    fn apply(&self, p: *mut pc_particle_t, count: c_uint, dt: f32) {
        if count == 0 {
            return;
        }
        let particles = unsafe { std::slice::from_raw_parts_mut(p, count as usize) };
        let soft2 = self.softening * self.softening;
        for q in particles.iter_mut() {
            let dx = self.x - q.position.x;
            let dy = self.y - q.position.y;
            let r2 = dx * dx + dy * dy + soft2;
            let inv_r3 = (r2.sqrt() * r2).recip();
            q.velocity.x += dx * self.strength * inv_r3 * dt;
            q.velocity.y += dy * self.strength * inv_r3 * dt;
        }
    }
}

extern "C" fn attractor_apply_thunk(data: *mut c_void, p: *mut pc_particle_t, count: c_uint, dt: f32) {
    let attractor = unsafe { &*(data as *const Attractor) };
    attractor.apply(p, count, dt)
}

#[pymethods]
impl Attractor {
    #[new]
    #[pyo3(signature = (x = 400.0, y = 300.0, strength = 5000.0, softening = 20.0))]
    fn new(x: f32, y: f32, strength: f32, softening: f32) -> Self {
        Attractor { x, y, strength, softening }
    }

    fn c_interface(slf: PyRef<'_, Self>) -> (usize, usize, usize) {
        (
            &*slf as *const Self as usize,
            attractor_apply_thunk as usize,
            0,
        )
    }
}

// ===========================================================================
// PyO3 module
// ===========================================================================

#[pymodule]
fn particle_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Ring>()?;
    m.add_class::<Vortex>()?;
    m.add_class::<Attractor>()?;
    m.add(
        "EMITTERS",
        pyo3::types::PyTuple::new(m.py(), [m.getattr("Ring")?])?,
    )?;
    m.add(
        "MODIFIERS",
        pyo3::types::PyTuple::new(m.py(), [m.getattr("Vortex")?, m.getattr("Attractor")?])?,
    )?;
    Ok(())
}
