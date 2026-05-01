#include <particle_cpp/types.h>

#include <cmath>
#include <cstdint>

namespace particle_cpp {

namespace {

extern "C" void modifier_apply_thunk(void *data, pc_particle_t *p, uint32_t count, float dt) {
    static_cast<Modifier *>(data)->apply(p, count, dt);
}

}  // namespace

pc_modifier_t Modifier::as_c_modifier() {
    return pc_modifier_t{
        /*.data    =*/ this,
        /*.apply   =*/ modifier_apply_thunk,
        /*.destroy =*/ nullptr,
    };
}

void Gravity::apply(pc_particle_t *p, uint32_t count, float dt) {
    for (uint32_t i = 0; i < count; ++i) {
        p[i].velocity.x += ax * dt;
        p[i].velocity.y += ay * dt;
    }
}

void Drag::apply(pc_particle_t *p, uint32_t count, float dt) {
    float decay = std::exp(-k * dt);
    for (uint32_t i = 0; i < count; ++i) {
        p[i].velocity.x *= decay;
        p[i].velocity.y *= decay;
    }
}

}  // namespace particle_cpp
