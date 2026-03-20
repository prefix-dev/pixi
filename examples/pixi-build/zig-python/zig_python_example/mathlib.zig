export fn add(a: i64, b: i64) i64 {
    return a + b;
}

export fn fibonacci(n: i64) i64 {
    if (n <= 0) return 0;
    if (n == 1) return 1;

    var a: i64 = 0;
    var b: i64 = 1;
    var i: i64 = 2;
    while (i <= n) : (i += 1) {
        const tmp = a + b;
        a = b;
        b = tmp;
    }
    return b;
}

export fn gcd(a_arg: i64, b_arg: i64) i64 {
    var a = if (a_arg < 0) -a_arg else a_arg;
    var b = if (b_arg < 0) -b_arg else b_arg;
    while (b != 0) {
        const t = b;
        b = @mod(a, b);
        a = t;
    }
    return a;
}
