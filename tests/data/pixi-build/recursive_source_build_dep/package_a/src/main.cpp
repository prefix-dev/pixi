#include <iostream>
#include <package_b.h>

int main() {
    std::cout << "Package A application starting..." << std::endl;

    // Use the add function from package_b
    int a = 5;
    int b = 3;
    int result = package_b::add(a, b);
    std::cout << a << " + " << b << " = " << result << std::endl;
    std::cout << "Package A application finished!" << std::endl;

    return 0;
}
