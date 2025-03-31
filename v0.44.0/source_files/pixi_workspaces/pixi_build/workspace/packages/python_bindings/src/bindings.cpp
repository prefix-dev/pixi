#include <nanobind/nanobind.h>

int add(int a, int b) { return a + b; } // (1)!

NB_MODULE(python_bindings, m)
{
    m.def("add", &add); // (2)!
}
