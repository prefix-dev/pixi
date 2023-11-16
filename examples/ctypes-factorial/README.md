# Ctypes example: large factorials

This examples calculates approximate factorials of large numbers using pure Python or Python with ctypes.


## Features demonstrated:
- packaging C and Python libraries together
- compiling with gcc through Pixi tasks
- conditional Pixi task execution (`depends_on`)
- defining tasks with arguments


## Usage

C implementation used in Python via ctypes was about 5x faster than pure python on the author's machine.  How fast was your setup?

```
PS C:\Users\a\Desktop\code\pixi\examples\ctypes-factorial> pixi run start
2023-11-11@21:30:24.650235|INFO|__main__.<module>:69|calculating factorial of 12345678 using ctypes...
2023-11-11@21:30:25.124916|INFO|__main__.<module>:77|12345678! ≈ 1.457260e82187904
2023-11-11@21:30:25.356799|INFO|__main__.<module>:66|calculating factorial of 12345678 using pure Python...
2023-11-11@21:30:32.065728|INFO|__main__.<module>:77|12345678! ≈ 1.457260e82187904
```

Run in with Python engine (does not depend on compiling C lib):
```
PS C:\Users\a\Desktop\code\pixi\examples\ctypes-factorial> pixi run factorial
2023-11-11@21:34:24.741955|INFO|__main__.<module>:66|calculating factorial of 10 using pure Python...
2023-11-11@21:34:24.741955|INFO|__main__.<module>:77|10! ≈ 3.628800e6
```

In the Pixi task `pixi run factorial`, the default `n` is not set, and the default engine is `python`.
In the python script, the default `n` is 10, and the default is `ctypes`.
Arguments passed into `pixi run` will propagate to its underlying task, and play nicely with defaults:

```
pixi run factorial  # python script defines default n=10
pixi run factorial 100  # overrides default Python n
pixi run factorial -e ctypes  # overrides default Python engine
pixi run factorial 100 -e ctypes  # overrides both defaults
```
