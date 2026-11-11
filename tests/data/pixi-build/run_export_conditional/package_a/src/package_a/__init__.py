import subprocess


def main() -> None:
    print("Package A application starting")
    subprocess.run("package-b", check=True, shell=True)
