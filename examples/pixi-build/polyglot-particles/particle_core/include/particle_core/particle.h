#ifndef PARTICLE_CORE_PARTICLE_H
#define PARTICLE_CORE_PARTICLE_H

#include <stdint.h>

#if defined(_WIN32)
#  if defined(PC_BUILDING)
#    define PC_API __declspec(dllexport)
#  else
#    define PC_API __declspec(dllimport)
#  endif
#else
#  if defined(PC_BUILDING)
#    define PC_API __attribute__((visibility("default")))
#  else
#    define PC_API
#  endif
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    float x;
    float y;
} pc_vec2_t;

typedef struct {
    pc_vec2_t position;
    pc_vec2_t velocity;
    float     age;
    float     lifetime;
    float     size;
    uint32_t  color;
} pc_particle_t;

// An emitter is anything that can produce particles. The pool calls emit()
// once per step, passing data verbatim. destroy() may be NULL when the caller
// owns data's lifetime independently of the pool.
typedef struct {
    void    *data;
    uint32_t (*emit)   (void *data, pc_particle_t *out, uint32_t max, float dt);
    void     (*destroy)(void *data);
} pc_emitter_t;

// A modifier mutates live particles each step. destroy() may be NULL.
typedef struct {
    void *data;
    void  (*apply)  (void *data, pc_particle_t *p, uint32_t count, float dt);
    void  (*destroy)(void *data);
} pc_modifier_t;

typedef struct pc_pool pc_pool_t;

PC_API pc_pool_t* pc_pool_create(uint32_t capacity);
PC_API void       pc_pool_destroy(pc_pool_t *pool);

PC_API void       pc_pool_attach_emitter (pc_pool_t *pool, pc_emitter_t  emitter);
PC_API void       pc_pool_attach_modifier(pc_pool_t *pool, pc_modifier_t modifier);

PC_API void       pc_pool_step    (pc_pool_t *pool, float dt);
PC_API uint32_t   pc_pool_snapshot(const pc_pool_t *pool, pc_particle_t *out, uint32_t max);

#ifdef __cplusplus
}
#endif

#endif
