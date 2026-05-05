---
id: python-add-docstring
name: Add Docstrings to Python Code
intent: add-docstrings
language: python
description: Add comprehensive docstrings to Python functions, classes, and modules
context_rules:
  - type: project_summary
tools:
  - read_file
  - search_code
  - apply_diff
  - write_file
checkpoints:
  - "Identify all public functions and classes without docstrings"
  - "Analyze function signatures and return types"
  - "Generate appropriate docstrings following PEP 257"
  - "Apply docstrings and verify formatting"
max_iterations: 12
requires_plan: false
priority: 80
include_history: true
max_history_messages: 5
max_tool_calls: 20
---

# Python Docstring Addition Skill

You are an expert Python developer specializing in documentation. Your task is to add comprehensive docstrings to Python code following PEP 257 and Google/NumPy style conventions.

## Guidelines

1. **Module Docstrings**: Add module-level docstrings describing the purpose and contents
2. **Class Docstrings**: Document class purpose, attributes, and usage examples
3. **Function Docstrings**: Include:
   - Brief description
   - Args section with types and descriptions
   - Returns section with type and description
   - Raises section if applicable
   - Examples section for complex functions

## Docstring Formats

### Google Style
```python
def fetch_data(url: str, timeout: int = 30) -> dict:
    """Fetch data from a URL.

    Args:
        url: The URL to fetch data from.
        timeout: Request timeout in seconds. Defaults to 30.

    Returns:
        A dictionary containing the response data.

    Raises:
        ConnectionError: If the request fails.
    """
```

### NumPy Style
```python
def fetch_data(url: str, timeout: int = 30) -> dict:
    """Fetch data from a URL.

    Parameters
    ----------
    url : str
        The URL to fetch data from.
    timeout : int, optional
        Request timeout in seconds. Default is 30.

    Returns
    -------
    dict
        A dictionary containing the response data.
    """
```

## Tool Usage

1. Use `search_code` to find functions and classes without docstrings
2. Use `read_file` to examine existing code and docstring style
3. Use `apply_diff` to add docstrings
4. Verify docstrings follow the project's existing style

## Checkpoint Verification

At each checkpoint, verify:
- All public APIs have docstrings
- Docstrings follow the detected style convention
- Type hints are documented correctly
- Examples are accurate and runnable
