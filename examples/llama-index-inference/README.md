# LLM Inference with `llama-index` and `llama.cpp`

This AI / Machine Learning example shows how to run LLM inference from a local model downloaded from HuggingFace. It uses a mix of `conda-forge` packages and PyPi packages to get the proper, compatible versions.

Run the example with:

```bash
$ pixi install
$ pixi run start
```

The source code is derived from [the Llama Index documentation](https://docs.llamaindex.ai/en/stable/examples/llm/llama_2_llama_cpp/). This particular set of tools and libraries was selected to show that production-grade deployments are possible with Pixi. The selected libraries in here are fairly lightweight and run a very advanced model locally. This was the performance I received on my local M1 Max machine:

```bash
llama_print_timings:        load time =    2043.14 ms
llama_print_timings:      sample time =      22.51 ms /   247 runs   (    0.09 ms per token, 10973.88 tokens per second)
llama_print_timings: prompt eval time =    2043.03 ms /    71 tokens (   28.78 ms per token,    34.75 tokens per second)
llama_print_timings:        eval time =   17786.87 ms /   246 runs   (   72.30 ms per token,    13.83 tokens per second)
llama_print_timings:       total time =   19959.11 ms /   317 tokens
```

Opportunities for improvement:

- Modify for Linux / CUDA environments to demonstrate a more practical production stack.
- Enhance the pipeline with a RAG workflow, which is what Llama Index is good at.
- Experiment with different GGUF models for a quality / performance balance that fits your hardware.
