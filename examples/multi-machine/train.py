import os
import sys

if os.environ["PIXI_ENVIRONMENT_NAME"] == "mlx":
    import mlx.core as mx

    a = mx.array([1, 2, 3, 4])
    print(a.shape)
    print("MLX is available, in mlx environment as expected")

if os.environ["PIXI_ENVIRONMENT_NAME"] == "cuda":
    import torch

    assert torch.cuda.is_available(), "CUDA is not available"
    print("CUDA is available, in cuda environment as expected")

if os.environ["PIXI_ENVIRONMENT_NAME"] == "default":
    import torch

    assert not torch.cuda.is_available(), "CUDA is available, in default environment"
    print("CUDA is not available, in default environment as expected")

print("\nHello from train.py!")
print("Environment you are running on:")
print(os.environ["PIXI_ENVIRONMENT_NAME"])
print("Arguments given to the script:")
print(sys.argv[1:])
