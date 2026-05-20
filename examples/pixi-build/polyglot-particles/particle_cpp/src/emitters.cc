#include <particle_cpp/types.h>

#include <cmath>
#include <cstdint>

namespace particle_cpp {

namespace {

uint32_t xorshift32(uint32_t *s) {
    uint32_t x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = x ? x : 1u;
    return *s;
}

float frandf(uint32_t *s, float lo, float hi) {
    return lo + (xorshift32(s) / (float)UINT32_MAX) * (hi - lo);
}

extern "C" uint32_t emitter_emit_thunk(void *data, pc_particle_t *out, uint32_t max, float dt) {
    return static_cast<Emitter *>(data)->emit(out, max, dt);
}

}  // namespace

pc_emitter_t Emitter::as_c_emitter() {
    return pc_emitter_t{
        /*.data    =*/ this,
        /*.emit    =*/ emitter_emit_thunk,
        /*.destroy =*/ nullptr,
    };
}

uint32_t Cone::emit(pc_particle_t *out, uint32_t max, float dt) {
    accumulator += rate * dt;
    uint32_t to_emit = (uint32_t)accumulator;
    if (to_emit > max) to_emit = max;
    accumulator -= (float)to_emit;

    for (uint32_t i = 0; i < to_emit; ++i) {
        float a = angle + frandf(&rng, -spread, spread);
        out[i].position = { x, y };
        out[i].velocity = { std::cos(a) * speed, std::sin(a) * speed };
        out[i].age      = 0.0f;
        out[i].lifetime = lifetime;
        out[i].size     = size;
        out[i].color    = color;
    }
    return to_emit;
}

uint32_t Burst::emit(pc_particle_t *out, uint32_t max, float dt) {
    (void)dt;
    if (fired) return 0;
    uint32_t n = count > max ? max : count;
    for (uint32_t i = 0; i < n; ++i) {
        float a = frandf(&rng, 0.0f, 6.2831853f);
        float v = frandf(&rng, 0.5f, 1.0f) * speed;
        out[i].position = { x, y };
        out[i].velocity = { std::cos(a) * v, std::sin(a) * v };
        out[i].age      = 0.0f;
        out[i].lifetime = lifetime;
        out[i].size     = size;
        out[i].color    = color;
    }
    fired = true;
    return n;
}

}  // namespace particle_cpp
