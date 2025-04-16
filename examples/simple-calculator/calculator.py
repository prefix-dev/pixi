#!/usr/bin/env python3
"""
A simple calculator module for demonstrating pixi task arguments.
This file contains functions that can be called by the calculate task.
"""


def sum(a, b):
    """Add two numbers and return the result."""
    return int(a) + int(b)


def multiply(a, b):
    """Multiply two numbers and return the result."""
    return int(a) * int(b)


def subtract(a, b):
    """Subtract b from a and return the result."""
    return int(a) - int(b)


def divide(a, b):
    """Divide a by b and return the result."""
    return int(a) / int(b)


if __name__ == "__main__":
    print("Calculator module loaded.")
    print("Available operations: sum, multiply, subtract, divide")
