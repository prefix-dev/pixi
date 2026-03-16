#include <stdio.h>

void calculate_factorial_approximation(unsigned long n, double *significand, int *exponent) {
    if (n <= 1) {
        *significand = 1.0;
        *exponent = 0;
        return;
    }

    *significand = 1.0;
    *exponent = 0;

    for (unsigned long i = 2; i <= n; i++) {
        *significand *= (double)i;

        // Normalize the significand
        while (*significand >= 10.0) {
            *significand /= 10.0;
            (*exponent)++;
        }
    }
}
