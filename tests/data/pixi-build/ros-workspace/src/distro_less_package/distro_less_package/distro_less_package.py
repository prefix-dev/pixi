# Simple hello world script for ROS2 in Python
from typing import Any

import rclpy  # type: ignore[import-not-found]


def main(args: Any = None) -> None:
    rclpy.init(args=args)
    print("Distroless package")
