#!/usr/bin/env python
# /// conda-script
# [dependencies]
# python = "3.12.*"
# requests = "*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "python"
# /// end-conda-script

"""
Fetch and display information from the GitHub API.

Demonstrates using external dependencies (requests) with conda-script.

Run with: pixi exec web_request.py
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
