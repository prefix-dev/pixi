#include <iostream>
#include <SDL.h>

int main( int argc, char* args[] ) {
    if (argc > 1 && std::string(args[1]) == "-h") {
        std::cout << "Usage: sdl-example [options]\n"
                  << "A simple SDL example that creates a window and draws a square that follows the mouse cursor.\n"
                  << "Options:\n"
                  << "  -h    Show this help message\n";
        return 0;
    }

    // Initialize SDL
    if( SDL_Init( SDL_INIT_VIDEO ) < 0 )
    {
        std::cout << "SDL could not initialize! SDL_Error: " << SDL_GetError() << std::endl;
        return 1;
    }

    // Create window
    SDL_Window *window = SDL_CreateWindow("Basic Pixi SDL project",
                                          SDL_WINDOWPOS_UNDEFINED,
                                          SDL_WINDOWPOS_UNDEFINED,
                                          800, 600,
                                          SDL_WINDOW_SHOWN);
    if(window == nullptr) {
        std::cout << "Failed to create SDL window (error" << SDL_GetError() << ")" << std::endl;
        SDL_Quit();
        return 1;
    }

    SDL_Renderer *renderer = SDL_CreateRenderer(window, -1, SDL_RENDERER_ACCELERATED);
    if(renderer == nullptr) {
        std::cout << "Failed to create SDL renderer (error" << SDL_GetError() << ")" << std::endl;
        SDL_DestroyWindow(window);
        SDL_Quit();
        return 1;
    }

    // Declare rect of square
    SDL_Rect squareRect;

    // Square dimensions: Half of the min(SCREEN_WIDTH, SCREEN_HEIGHT)
    squareRect.w = 300;
    squareRect.h = 300;

    // Event loop exit flag
    bool quit = false;

    // Event loop
    while(!quit)
    {
        SDL_Event e;

        // Wait indefinitely for the next available event
        SDL_WaitEvent(&e);

        // User requests quit
        if(e.type == SDL_QUIT)
        {
            quit = true;
        }

	// Get mouse position
	int mouseX, mouseY;
	SDL_GetMouseState(&mouseX, &mouseY);

	// Update square position to follow the mouse cursor
	squareRect.x = mouseX - squareRect.w / 2;
	squareRect.y = mouseY - squareRect.h / 2;


        // Initialize renderer color white for the background
        SDL_SetRenderDrawColor(renderer, 0xFF, 0xFF, 0xFF, 0xFF);

        // Clear screen
        SDL_RenderClear(renderer);

        // Set renderer color red to draw the square
        SDL_SetRenderDrawColor(renderer, 0xFF, 0x00, 0x00, 0xFF);

        // Draw a rectangle
        SDL_RenderFillRect(renderer, &squareRect);

        // Update screen
        SDL_RenderPresent(renderer);
    }

    SDL_DestroyRenderer(renderer);
    SDL_DestroyWindow(window);
    SDL_Quit();

    return 0;
}
