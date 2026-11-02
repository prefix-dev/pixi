import json
import os


def main():
    username_variables = ("LOGNAME", "USER", "LNAME", "USERNAME")
    username = next(
        (os.environ[name] for name in username_variables if os.environ.get(name)),
        None,
    )
    if username is None:
        raise RuntimeError("username is not available")
    print(
        json.dumps(
            {
                "version": 1,
                "virtual_packages": {
                    "__username_length": {"version": str(len(username)), "build_string": "0"}
                },
                "cache": {"watch_env": username_variables},
            }
        )
    )
