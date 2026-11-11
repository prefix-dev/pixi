#ifndef PARTICLE_CPP_TYPES_H
#define PARTICLE_CPP_TYPES_H

#include <particle_core/particle.h>

#include <cstdint>

#if defined(_WIN32)
#  if defined(PCPP_BUILDING)
#    define PCPP_API __declspec(dllexport)
#  else
#    define PCPP_API __declspec(dllimport)
#  endif
#else
#  if defined(PCPP_BUILDING)
#    define PCPP_API __attribute__((visibility("default")))
#  else
#    define PCPP_API
#  endif
#endif

namespace particle_cpp {

struct PCPP_API Emitter {
    virtual ~Emitter() = default;
    virtual uint32_t emit(pc_particle_t *out, uint32_t max, float dt) = 0;

    // Returns a C ABI handle that borrows this object's lifetime.
    // destroy is null; the caller (pybind11 wrapper, etc.) owns *this.
    pc_emitter_t as_c_emitter();
};

struct PCPP_API Modifier {
    virtual ~Modifier() = default;
    virtual void apply(pc_particle_t *p, uint32_t count, float dt) = 0;

    pc_modifier_t as_c_modifier();
};

struct PCPP_API Cone : Emitter {
    float    x           = 400.0f;
    float    y           = 520.0f;
    float    angle       = -1.5707963f;
    float    spread      = 0.4f;
    float    speed       = 180.0f;
    float    rate        = 150.0f;
    float    lifetime    = 2.0f;
    float    size        = 3.0f;
    uint32_t color       = 0xFFAA22FFu;
    uint32_t rng         = 0xCAFEBABEu;
    float    accumulator = 0.0f;

    uint32_t emit(pc_particle_t *out, uint32_t max, float dt) override;
};

struct PCPP_API Burst : Emitter {
    float    x        = 400.0f;
    float    y        = 300.0f;
    uint32_t count    = 200u;
    float    speed    = 150.0f;
    float    lifetime = 1.5f;
    float    size     = 3.0f;
    uint32_t color    = 0xFFFFFFFFu;
    uint32_t rng      = 0xDEADBEEFu;
    bool     fired    = false;

    uint32_t emit(pc_particle_t *out, uint32_t max, float dt) override;
};

struct PCPP_API Gravity : Modifier {
    float ax = 0.0f;
    float ay = 200.0f;

    void apply(pc_particle_t *p, uint32_t count, float dt) override;
};

struct PCPP_API Drag : Modifier {
    float k = 0.5f;

    void apply(pc_particle_t *p, uint32_t count, float dt) override;
};

}  // namespace particle_cpp

#endif
