from .fast_math import sum_as_string  # type: ignore


def main() -> None:
    print(f"3 + 5 = {sum_as_string(3, 5)}")


if __name__ == "__main__":
    main()
