#include <nanobind/nanobind.h>

int add(int a, int b) { return a + b; } // (1)!

NB_MODULE(cpp_math, m)
{
    m.def("add", &add); // (2)!
}
