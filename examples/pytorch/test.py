import torch

if torch.cuda.is_available():
    print("CUDA is available")
else:
    print("CUDA is not available")
