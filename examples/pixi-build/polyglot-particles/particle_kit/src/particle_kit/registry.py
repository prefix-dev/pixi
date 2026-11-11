import particle_cpp_py as cpp
import particle_rs as rs


def all_emitters() -> dict[str, tuple[type, ...]]:
    return {
        "particle_cpp_py": cpp.EMITTERS,
        "particle_rs": rs.EMITTERS,
    }


def all_modifiers() -> dict[str, tuple[type, ...]]:
    return {
        "particle_cpp_py": cpp.MODIFIERS,
        "particle_rs": rs.MODIFIERS,
    }


def print_all() -> None:
    print("Emitters:")
    for pkg, classes in all_emitters().items():
        for cls in classes:
            print(f"  {pkg}.{cls.__name__}")
    print("Modifiers:")
    for pkg, classes in all_modifiers().items():
        for cls in classes:
            print(f"  {pkg}.{cls.__name__}")
