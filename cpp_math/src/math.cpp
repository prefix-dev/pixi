#include <nanobind/nanobind.h>

int add(int a, int b) { return a + b; }

NB_MODULE(cpp_math, m)
{
    m.def("add", &add);
}
