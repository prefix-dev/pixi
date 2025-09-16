import tomllib
import tomli_w

from dataclasses import dataclass, field
from filelock import FileLock
from pathlib import Path
from typing import Any


# Path to the results file, containing test outcomes
RESULTS_FILE = Path(__file__).parent / ".wheel_test_results.toml"
# Lock file to ensure process-safe write access to the results file
LOCK_FILE = RESULTS_FILE.with_suffix(".lock")


@dataclass
class Test:
    id: str
    results: list[dict[str, Any]] = field(default_factory=list)


def record_result(test_id: str, name: str, outcome: str, duration: float, details: str) -> None:
    """
    Collects test status after each test run, compatible with pytest-xdist.
    """
    result = {"name": name, "outcome": outcome, "duration": duration, "longrepr": details}

    # Use file lock for process-safe write access to the results file
    lock = FileLock(str(LOCK_FILE))

    with lock:
        test = Test(id=test_id)

        # Get the existing results
        if RESULTS_FILE.exists():
            with RESULTS_FILE.open("rb") as f:
                data = tomllib.load(f)
                # If this doesn't hold, don't use the recorded data
                if "id" in data and data["id"] == test_id:
                    test = Test(id=data["id"], results=data["results"])

        # Append the new result
        # if we are in the same session
        if test.id == test_id:
            test.results.append(result)
        # The data is from a different session
        # so we overwrite the data
        else:
            test.results = [result]

        # Write the results back to the file
        with RESULTS_FILE.open("wb") as f:
            tomli_w.dump(test.__dict__, f)
