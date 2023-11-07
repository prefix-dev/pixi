# Ctypes example: large factorials

This examples calculates approximate factorials of large numbers using pure Python or Python with ctypes.


## Features demonstrated:
- packaging C and Python libraries together
- compiling with gcc through Pixi tasks
- conditional Pixi task execution (`depends_on`)
- defining tasks with arguments


## Output

C implementation used in Python via ctypes was about 5x faster than pure python on the author's machine.  How fast was your setup?

```
$ pixi run factorial 12345678 python
2023-11-06@18:49:19.296216|INFO|__main__.<module>:50|calculating approximate factorial of 12345678 using pure Python...
2023-11-06@18:49:23.992089|INFO|__main__.<module>:61|12345678! ≈ 1.457260e82187904

$ pixi run factorial 12345678 ctypes
2023-11-06@18:49:29.404679|INFO|__main__.<module>:53|calculating factorial of 12345678 using ctypes...
2023-11-06@18:49:29.947188|INFO|__main__.<module>:61|12345678! ≈ 1.457260e82187904
```
