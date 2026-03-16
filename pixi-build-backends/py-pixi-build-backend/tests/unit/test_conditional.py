from pixi_build_backend.types.conditional import ConditionalString, ListOrItemString


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
    assert conditional_dep.then_value[0] == "package1"

    # Test extending the then_value list
    result = conditional_dep.then_value.extend(["package2", "package3"])
    assert result is None  # extend() returns None
    assert len(conditional_dep.then_value) == 3
    assert conditional_dep.then_value[1] == "package2"
    assert conditional_dep.then_value[2] == "package3"

    # Test item assignment
    conditional_dep.then_value[0] = "package001"
    assert conditional_dep.then_value[0] == "package001"

    # Test item deletion
    del conditional_dep.then_value[1]  # Remove "package2"
    assert len(conditional_dep.then_value) == 2
    assert conditional_dep.then_value[0] == "package001"
    assert conditional_dep.then_value[1] == "package3"

    # Test final state matches expected
    expected_then = ListOrItemString(["package001", "package3"])
    assert conditional_dep.then_value == expected_then

    # Test else_value is accessible and equals initial value
    assert conditional_dep.else_value == else_
