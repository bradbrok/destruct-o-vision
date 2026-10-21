# Security

Destruct-O-Vision opens patch files and will load modules that other people share, so we treat those as untrusted input. Please report anything that lets one of them:
- crash the app
- hang the GPU
- use unbounded memory
- read or write files
- run code

## Reporting

Report vulnerabilities privately through GitHub: **Security → Report a vulnerability** on this repository. Please don't open a public issue. Include a patch or module that reproduces the problem if you can.

We'll acknowledge the report and keep you posted on the fix.
