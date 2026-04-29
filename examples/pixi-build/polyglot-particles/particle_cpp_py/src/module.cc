#include <pybind11/pybind11.h>
#include <pybind11/stl.h>

#include <particle_cpp/types.h>

#include <cstdint>
#include <tuple>

namespace py = pybind11;
using particle_cpp::Burst;
using particle_cpp::Cone;
using particle_cpp::Drag;
using particle_cpp::Emitter;
using particle_cpp::Gravity;
using particle_cpp::Modifier;

namespace {

const Cone    kCone;
const Burst   kBurst;
const Gravity kGravity;
const Drag    kDrag;

using EmitterTuple  = std::tuple<uintptr_t, uintptr_t, uintptr_t>;
using ModifierTuple = std::tuple<uintptr_t, uintptr_t, uintptr_t>;

EmitterTuple emitter_c_interface(Emitter &self) {
    pc_emitter_t e = self.as_c_emitter();
    return {
        reinterpret_cast<uintptr_t>(e.data),
        reinterpret_cast<uintptr_t>(e.emit),
        reinterpret_cast<uintptr_t>(e.destroy),
    };
}

ModifierTuple modifier_c_interface(Modifier &self) {
    pc_modifier_t m = self.as_c_modifier();
    return {
        reinterpret_cast<uintptr_t>(m.data),
        reinterpret_cast<uintptr_t>(m.apply),
        reinterpret_cast<uintptr_t>(m.destroy),
    };
}

}  // namespace

PYBIND11_MODULE(_native, m) {
    m.doc() = "C++ particle emitter/modifier classes";

    py::class_<Emitter>(m, "Emitter")
        .def("c_interface", &emitter_c_interface);

    py::class_<Modifier>(m, "Modifier")
        .def("c_interface", &modifier_c_interface);

    py::class_<Cone, Emitter>(m, "Cone")
        .def(py::init([](float x, float y, float angle, float spread,
                         float speed, float rate, float lifetime, float size,
                         uint32_t color, uint32_t rng) {
                 Cone c;
                 c.x = x; c.y = y; c.angle = angle; c.spread = spread;
                 c.speed = speed; c.rate = rate; c.lifetime = lifetime; c.size = size;
                 c.color = color; c.rng = rng;
                 return c;
             }),
             py::arg("x")        = kCone.x,
             py::arg("y")        = kCone.y,
             py::arg("angle")    = kCone.angle,
             py::arg("spread")   = kCone.spread,
             py::arg("speed")    = kCone.speed,
             py::arg("rate")     = kCone.rate,
             py::arg("lifetime") = kCone.lifetime,
             py::arg("size")     = kCone.size,
             py::arg("color")    = kCone.color,
             py::arg("rng")      = kCone.rng)
        .def_readwrite("x",        &Cone::x)
        .def_readwrite("y",        &Cone::y)
        .def_readwrite("angle",    &Cone::angle)
        .def_readwrite("spread",   &Cone::spread)
        .def_readwrite("speed",    &Cone::speed)
        .def_readwrite("rate",     &Cone::rate)
        .def_readwrite("lifetime", &Cone::lifetime)
        .def_readwrite("size",     &Cone::size)
        .def_readwrite("color",    &Cone::color)
        .def_readwrite("rng",      &Cone::rng);

    py::class_<Burst, Emitter>(m, "Burst")
        .def(py::init([](float x, float y, uint32_t count, float speed,
                         float lifetime, float size, uint32_t color, uint32_t rng) {
                 Burst b;
                 b.x = x; b.y = y; b.count = count; b.speed = speed;
                 b.lifetime = lifetime; b.size = size; b.color = color; b.rng = rng;
                 return b;
             }),
             py::arg("x")        = kBurst.x,
             py::arg("y")        = kBurst.y,
             py::arg("count")    = kBurst.count,
             py::arg("speed")    = kBurst.speed,
             py::arg("lifetime") = kBurst.lifetime,
             py::arg("size")     = kBurst.size,
             py::arg("color")    = kBurst.color,
             py::arg("rng")      = kBurst.rng)
        .def_readwrite("x",        &Burst::x)
        .def_readwrite("y",        &Burst::y)
        .def_readwrite("count",    &Burst::count)
        .def_readwrite("speed",    &Burst::speed)
        .def_readwrite("lifetime", &Burst::lifetime)
        .def_readwrite("size",     &Burst::size)
        .def_readwrite("color",    &Burst::color)
        .def_readwrite("rng",      &Burst::rng);

    py::class_<Gravity, Modifier>(m, "Gravity")
        .def(py::init([](float ax, float ay) {
                 Gravity g;
                 g.ax = ax; g.ay = ay;
                 return g;
             }),
             py::arg("ax") = kGravity.ax,
             py::arg("ay") = kGravity.ay)
        .def_readwrite("ax", &Gravity::ax)
        .def_readwrite("ay", &Gravity::ay);

    py::class_<Drag, Modifier>(m, "Drag")
        .def(py::init([](float k) {
                 Drag d;
                 d.k = k;
                 return d;
             }),
             py::arg("k") = kDrag.k)
        .def_readwrite("k", &Drag::k);

    m.attr("EMITTERS")  = py::make_tuple(m.attr("Cone"), m.attr("Burst"));
    m.attr("MODIFIERS") = py::make_tuple(m.attr("Gravity"), m.attr("Drag"));
}
