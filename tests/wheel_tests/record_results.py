from pathlib import Path
from multiprocessing import Lock
import toml
from filelock import FileLock

lock = Lock()
RESULTS_FILE = Path(__file__).parent / ".wheel_test_results.toml"
LOCK_FILE = RESULTS_FILE.with_suffix(".lock")


def record_result(test_id: str, name: str, outcome: str, duration: float, details: str):
    """
    Collects test status after each test run, compatible with pytest-xdist.
    """
    result = {"name": name, "outcome": outcome, "duration": duration, "longrepr": details}

    # Use file lock for process-safe write access to the results file
    lock = FileLock(str(LOCK_FILE))

    with lock:
        test = {"id": test_id, "results": []}

        # Get the existing results
        if RESULTS_FILE.exists():
            with RESULTS_FILE.open("r") as f:
                data = toml.load(f)
                # If this doesn't hold, don't use the recorded data
                if "id" in data and data["id"] == test_id:
                    test = data

        # Append the new result
        # if we are in the same session
        if test["id"] == test_id:
            test["results"].append(result)
        # The data is from a different session
        # so we overwrite the data
        else:
            test["results"] = [result]

        # Write the results back to the file
        with RESULTS_FILE.open("w") as f:
            toml.dump(test, f)
