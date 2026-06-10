# AWS Lambda

`AWS Lambda` is a serverless compute platform that lets you run code without managing servers. Pixi can be used to manage the Python environment and dependencies required for Lambda functions.

## Create a project

Create a new Pixi project:

```bash
pixi init my_lambda_project
```

## Add dependencies

Add Python:

```bash
pixi add python
```

Add AWS Lambda Powertools:

```bash
pixi add aws-lambda-powertools
```

Your `pixi.toml` should contain:

```toml title="pixi.toml"
[dependencies]
python = ">=3.13,<3.14"
aws-lambda-powertools = ">=3.29.0,<4"
```

## Create a Lambda handler

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

## Run locally

```bash
pixi run python main.py
```

Output:

```bash
{'statusCode': 200, 'body': 'Hello from AWS Lambda using Pixi'}
```

The output is the dictionary returned by the Lambda handler when invoked locally with a dummy event (`{}`) and no execution context (`None`).

## Deployment

### Build a container image

The following Dockerfile uses Pixi to create the Python environment in a build stage and copies it into an AWS Lambda Python runtime image.

```dockerfile title="Dockerfile"
FROM ghcr.io/prefix-dev/pixi:latest AS build

WORKDIR /app

COPY . .

RUN pixi install --locked

## Create an AWS-runtime Lambda image
FROM public.ecr.aws/lambda/python:3.13

# Copy the runtime dependencies from the builder stage.
COPY --from=build /app/.pixi/envs/default /opt/pixi-env

# Copy the application code.
COPY app/ ${LAMBDA_TASK_ROOT}/app

ENV PATH="/opt/pixi-env/bin:${PATH}"
ENV PYTHONPATH="/opt/pixi-env/lib/python3.13/site-packages:${PYTHONPATH}"

# Set the AWS Lambda handler.
CMD ["app.main.handler"]
```

Build the image:

```bash
docker build -t pixi-lambda .
```

### Test the image

Enter the container:

```bash
docker run -it --entrypoint /bin/sh pixi-lambda
```

Run the handler:

```bash
python /var/task/app/main.py
```

Output:

```bash
{'statusCode': 200, 'body': 'Hello from AWS Lambda using Pixi'}
```

## Further reading

* Deploying Pixi projects to production: https://tech.quantco.com/blog/pixi-production
* AWS Lambda documentation: https://docs.aws.amazon.com/lambda/latest/dg/welcome.html
