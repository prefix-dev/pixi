#include <pybind11/pybind11.h>

namespace py = pybind11;

double add(double a, double b) { return a + b; }

PYBIND11_MODULE(mysum, m) {
  m.doc() = "pybind11 example plugin"; // optional module docstring

  m.def("add", &add, "A function which adds two numbers");
}
