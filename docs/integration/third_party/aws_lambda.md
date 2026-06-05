AWS Lambda is a serverless compute platform that lets you run code without managing servers. Pixi can be used to manage the Python environment and dependencies required for Lambda functions.

## Create a project
Create a new Pixi project:

```bash
pixi init my_lambda_project
```

## Add dependencies

Add Python as a prerequisite:

```bash
pixi add python
```
Add AWS Lambda Powertools:

```bash
pixi add aws-lambda-powertools
```
AWS Lambda Powertools is a utility toolkit for AWS Lambda functions that provides logging, tracing, metrics, and other operational features.

Your pixi.toml should contain:
```toml title="pixi.toml"
[dependencies]
python = ">=3.14.5,<3.15"
aws-lambda-powertools = ">=3.29.0,<4"
```

Create a `main.py` file:

```python title="main.py"
from aws_lambda_powertools import Logger

logger = Logger()

def handler(event, context):
    logger.info("Request received")

    return {
        "statusCode": 200,
        "body": "Hello from AWS Lambda using Pixi"
    }

if __name__ == "__main__":
    print(handler({}, None))
```

Run the example:

```bash 
pixi run python main.py
```

Expected output:

```bash 
{'statusCode': 200, 'body': 'Hello from AWS Lambda using Pixi'}
```

The output is the dictionary returned by the Lambda handler when invoked locally with a dummy event (`{}`) and no execution context (`None`).