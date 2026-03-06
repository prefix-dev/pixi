#ifndef PACKAGE_B_H
#define PACKAGE_B_H

#ifdef _WIN32
    #ifdef BUILDING_DLL
        #define API_EXPORT __declspec(dllexport)
    #else
        #define API_EXPORT __declspec(dllimport)
    #endif
#else
    #define API_EXPORT __attribute__((visibility("default")))
#endif

namespace package_b {

// Simple function to add two integers
API_EXPORT int add(int a, int b);

} // namespace package_b

#endif // PACKAGE_B_H
