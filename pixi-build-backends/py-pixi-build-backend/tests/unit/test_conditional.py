from pixi_build_backend.types.conditional import ConditionalString, ListOrItemString
from pixi_build_backend.pixi_build_backend import PyItemString


def test_conditional_creation() -> None:
    """
    Test creation of a conditional package dependency.
    And validating that it can act as a List.

    """
    condition = "os == 'linux'"
    then = ListOrItemString(["package1"])
    else_ = ListOrItemString(["package3"])

    conditional_dep = ConditionalString(condition, then, else_)

    assert conditional_dep.condition == condition

    # Test that then_value contains the expected initial content
    assert len(conditional_dep.then_value) == 1
    assert str(conditional_dep.then_value[0]) == "package1"

    # Test extending the then_value list
    result = conditional_dep.then_value.extend([PyItemString("package2"), PyItemString("package3")])
    assert result is None  # extend() returns None
    assert len(conditional_dep.then_value) == 3
    assert str(conditional_dep.then_value[1]) == "package2"
    assert str(conditional_dep.then_value[2]) == "package3"

    # Test item assignment
    conditional_dep.then_value[0] = PyItemString("package001")
    assert str(conditional_dep.then_value[0]) == "package001"

    # Test item deletion
    del conditional_dep.then_value[1]  # Remove "package2"
    assert len(conditional_dep.then_value) == 2
    assert str(conditional_dep.then_value[0]) == "package001"
    assert str(conditional_dep.then_value[1]) == "package3"
