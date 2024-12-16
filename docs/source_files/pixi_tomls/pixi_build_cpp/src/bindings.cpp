#include <nanobind/nanobind.h>

int add(int a, int b) { return a + b; }

NB_MODULE(python_bindings, m)
{
    m.def("add", &add);
}
