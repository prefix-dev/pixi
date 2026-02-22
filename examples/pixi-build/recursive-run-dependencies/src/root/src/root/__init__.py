from depend import depend_text


def root_text() -> str:
    return f"Hello, world!\nAnd from depend: {depend_text()}"


print(root_text())
