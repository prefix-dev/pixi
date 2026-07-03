import subprocess


def main() -> None:
    print("Pixi Build is number 1")
    subprocess.run("package-b", check=True, shell=True)
