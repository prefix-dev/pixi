import os
import sys

print("Hello from test.py!")
print("Environment you are running on:")
print(os.environ["PIXI_ENVIRONMENT_NAME"])
print("Arguments given to the script:")
print(sys.argv[1:])
