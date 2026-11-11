#include "particle_core/particle.h"

#include <stdlib.h>
#include <string.h>

#define PC_MAX_ATTACHMENTS 64

struct pc_pool {
    pc_particle_t *particles;
    uint32_t       capacity;
    uint32_t       count;

    pc_emitter_t   emitters[PC_MAX_ATTACHMENTS];
    uint32_t       emitter_count;

    pc_modifier_t  modifiers[PC_MAX_ATTACHMENTS];
    uint32_t       modifier_count;
};

pc_pool_t *pc_pool_create(uint32_t capacity) {
    pc_pool_t *pool = (pc_pool_t *)calloc(1, sizeof(pc_pool_t));
    if (!pool) return NULL;

    pool->particles = (pc_particle_t *)calloc(capacity, sizeof(pc_particle_t));
    if (!pool->particles) {
        free(pool);
        return NULL;
    }
    pool->capacity = capacity;
    return pool;
}

void pc_pool_destroy(pc_pool_t *pool) {
    if (!pool) return;

    for (uint32_t i = 0; i < pool->emitter_count; ++i) {
        if (pool->emitters[i].destroy) {
            pool->emitters[i].destroy(pool->emitters[i].data);
        }
    }
    for (uint32_t i = 0; i < pool->modifier_count; ++i) {
        if (pool->modifiers[i].destroy) {
            pool->modifiers[i].destroy(pool->modifiers[i].data);
        }
    }
    free(pool->particles);
    free(pool);
}

void pc_pool_attach_emitter(pc_pool_t *pool, pc_emitter_t emitter) {
    if (!pool || !emitter.emit || pool->emitter_count >= PC_MAX_ATTACHMENTS) return;
    pool->emitters[pool->emitter_count++] = emitter;
}

void pc_pool_attach_modifier(pc_pool_t *pool, pc_modifier_t modifier) {
    if (!pool || !modifier.apply || pool->modifier_count >= PC_MAX_ATTACHMENTS) return;
    pool->modifiers[pool->modifier_count++] = modifier;
}

void pc_pool_step(pc_pool_t *pool, float dt) {
    if (!pool) return;

    uint32_t write = 0;
    for (uint32_t read = 0; read < pool->count; ++read) {
        pc_particle_t *p = &pool->particles[read];
        p->age += dt;
        if (p->age < p->lifetime) {
            p->position.x += p->velocity.x * dt;
            p->position.y += p->velocity.y * dt;
            if (write != read) pool->particles[write] = *p;
            ++write;
        }
    }
    pool->count = write;

    for (uint32_t i = 0; i < pool->modifier_count; ++i) {
        pool->modifiers[i].apply(pool->modifiers[i].data, pool->particles, pool->count, dt);
    }

    for (uint32_t i = 0; i < pool->emitter_count; ++i) {
        uint32_t free_slots = pool->capacity - pool->count;
        if (free_slots == 0) break;
        uint32_t produced = pool->emitters[i].emit(
            pool->emitters[i].data,
            &pool->particles[pool->count],
            free_slots,
            dt);
        pool->count += produced;
    }
}

uint32_t pc_pool_snapshot(const pc_pool_t *pool, pc_particle_t *out, uint32_t max) {
    if (!pool || !out) return 0;
    uint32_t n = pool->count < max ? pool->count : max;
    memcpy(out, pool->particles, (size_t)n * sizeof(pc_particle_t));
    return n;
}
