#!/usr/bin/env python
# /// script
# requires-python = ">=3.12"
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["requests"]
# ///

"""
Fetch and display information from the GitHub API.

Demonstrates conda dependencies (requests) declared under [tool.conda].

Run with: pixi exec web_request.py
Lock it for sharing with: pixi exec --lock web_request.py
"""

import requests


def main():
    print("Fetching GitHub API information...")

    # Fetch GitHub API root
    response = requests.get("https://api.github.com")

    print(f"\nStatus Code: {response.status_code}")
    print(f"Requests Version: {requests.__version__}")

    # Pretty print some of the response
    data = response.json()
    print("\nAvailable endpoints:")
    for key, value in list(data.items())[:5]:
        print(f"  {key}: {value}")


if __name__ == "__main__":
    main()
