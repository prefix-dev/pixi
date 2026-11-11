#include <pybind11/pybind11.h>
#include <pybind11/stl.h>

#include <SDL.h>
#include <particle_core/particle.h>

#include <algorithm>
#include <chrono>
#include <stdexcept>
#include <string>
#include <thread>
#include <tuple>
#include <vector>

namespace py = pybind11;

namespace {

using EmitterFn  = uint32_t(*)(void *, pc_particle_t *, uint32_t, float);
using ModifierFn = void    (*)(void *, pc_particle_t *, uint32_t, float);
using DestroyFn  = void    (*)(void *);

pc_emitter_t to_c_emitter(const py::object &obj) {
    auto [data, fn, destroy] =
        py::cast<std::tuple<uintptr_t, uintptr_t, uintptr_t>>(obj.attr("c_interface")());
    return pc_emitter_t{
        reinterpret_cast<void *>(data),
        reinterpret_cast<EmitterFn>(fn),
        reinterpret_cast<DestroyFn>(destroy),
    };
}

pc_modifier_t to_c_modifier(const py::object &obj) {
    auto [data, fn, destroy] =
        py::cast<std::tuple<uintptr_t, uintptr_t, uintptr_t>>(obj.attr("c_interface")());
    return pc_modifier_t{
        reinterpret_cast<void *>(data),
        reinterpret_cast<ModifierFn>(fn),
        reinterpret_cast<DestroyFn>(destroy),
    };
}

class View {
public:
    View(int width, int height, std::string title)
        : width_(width), height_(height), title_(std::move(title)) {}

    void run(const std::vector<py::object> &emitters,
             const std::vector<py::object> &modifiers,
             uint32_t capacity,
             int fps) {
        if (SDL_Init(SDL_INIT_VIDEO) != 0) {
            throw std::runtime_error(std::string("SDL_Init failed: ") + SDL_GetError());
        }
        struct SdlGuard { ~SdlGuard() { SDL_Quit(); } } sdl_guard;

        SDL_Window *window = SDL_CreateWindow(
            title_.c_str(),
            SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED,
            width_, height_, SDL_WINDOW_SHOWN);
        if (!window) throw std::runtime_error(std::string("SDL_CreateWindow: ") + SDL_GetError());

        SDL_Renderer *renderer = SDL_CreateRenderer(
            window, -1, SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
        if (!renderer) {
            SDL_DestroyWindow(window);
            throw std::runtime_error(std::string("SDL_CreateRenderer: ") + SDL_GetError());
        }
        SDL_SetRenderDrawBlendMode(renderer, SDL_BLENDMODE_BLEND);

        pc_pool_t *pool = pc_pool_create(capacity);
        if (!pool) {
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            throw std::runtime_error("pc_pool_create returned null");
        }

        for (auto const &e : emitters) {
            pc_pool_attach_emitter(pool, to_c_emitter(e));
        }
        for (auto const &m : modifiers) {
            pc_pool_attach_modifier(pool, to_c_modifier(m));
        }

        std::vector<pc_particle_t> snapshot(capacity);
        const auto frame_dur = std::chrono::duration<double>(1.0 / std::max(1, fps));
        auto prev = std::chrono::steady_clock::now();
        bool running = true;
        SDL_Event ev;

        py::gil_scoped_release release;

        while (running) {
            auto frame_start = std::chrono::steady_clock::now();
            float dt = std::chrono::duration<float>(frame_start - prev).count();
            if (dt > 0.1f) dt = 0.1f;
            prev = frame_start;

            while (SDL_PollEvent(&ev)) {
                if (ev.type == SDL_QUIT) running = false;
                if (ev.type == SDL_KEYDOWN && ev.key.keysym.sym == SDLK_ESCAPE) running = false;
            }

            pc_pool_step(pool, dt);
            uint32_t n = pc_pool_snapshot(pool, snapshot.data(), capacity);

            SDL_SetRenderDrawColor(renderer, 16, 16, 24, 255);
            SDL_RenderClear(renderer);

            for (uint32_t i = 0; i < n; ++i) {
                const pc_particle_t &p = snapshot[i];
                Uint8 r = (Uint8)((p.color >> 24) & 0xFF);
                Uint8 g = (Uint8)((p.color >> 16) & 0xFF);
                Uint8 b = (Uint8)((p.color >>  8) & 0xFF);
                Uint8 a = (Uint8)((p.color      ) & 0xFF);
                if (p.lifetime > 0.0f) {
                    float remaining = 1.0f - (p.age / p.lifetime);
                    if (remaining < 0.0f) remaining = 0.0f;
                    a = (Uint8)(a * remaining);
                }
                SDL_SetRenderDrawColor(renderer, r, g, b, a);
                int sz = (int)p.size;
                if (sz < 1) sz = 1;
                SDL_Rect rect = {
                    (int)p.position.x - sz / 2,
                    (int)p.position.y - sz / 2,
                    sz, sz
                };
                SDL_RenderFillRect(renderer, &rect);
            }
            SDL_RenderPresent(renderer);

            auto frame_elapsed = std::chrono::steady_clock::now() - frame_start;
            if (frame_elapsed < frame_dur) {
                std::this_thread::sleep_for(frame_dur - frame_elapsed);
            }
        }

        pc_pool_destroy(pool);
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
    }

private:
    int         width_;
    int         height_;
    std::string title_;
};

}  // namespace

PYBIND11_MODULE(_native, m) {
    m.doc() = "SDL2 visualizer for the particle pool";

    py::class_<View>(m, "View")
        .def(py::init<int, int, std::string>(),
             py::arg("width")  = 800,
             py::arg("height") = 600,
             py::arg("title")  = "Polyglot Particles")
        .def("run", &View::run,
             py::arg("emitters")  = std::vector<py::object>{},
             py::arg("modifiers") = std::vector<py::object>{},
             py::arg("capacity")  = 10000u,
             py::arg("fps")       = 60);
}
