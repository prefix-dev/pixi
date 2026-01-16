#!/usr/bin/env julia
# /// conda-script
# [dependencies]
# julia = "1.9.*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "julia"
# /// end-conda-script

# A simple Hello World Julia script demonstrating conda-script metadata
# Run with: pixi exec hello_julia.jl

println("=" ^ 60)
println("Hello from Julia with conda-script!")
println("=" ^ 60)
println("Julia version: ", VERSION)
println("Platform: ", Sys.KERNEL, " ", Sys.ARCH)
println("=" ^ 60)

# Simple Julia example
numbers = 1:10
println("Sum of 1 to 10: ", sum(numbers))
println("Mean of 1 to 10: ", sum(numbers) / length(numbers))
