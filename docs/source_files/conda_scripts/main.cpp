// /// conda-script
// channels = ["https://prefix.dev/conda-forge"]
// entrypoint = "g++ -o ${CACHE}/main ${SCRIPT} -lfmt && ${CACHE}/main"
//
// [dependencies]
// gxx = "*"
// fmt = "*"
// /// end-conda-script
#include <fmt/core.h>
#include <fmt/ranges.h>
#include <vector>

int main() {
    std::vector<int> primes{2, 3, 5, 7, 11};
    fmt::print("primes: {}\n", fmt::join(primes, ", "));
    fmt::print("pi is roughly {:.3f}\n", 3.14159);
}
