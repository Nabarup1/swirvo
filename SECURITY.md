# Security

Swirvo opens files from anywhere, so security reports matter.

## Reporting a problem

Please **don't** open a public issue for a security problem. Use GitHub's private reporting instead: go to the repository's **Security** tab and choose **Report a vulnerability**.

Include the Swirvo version, your Windows version, what happens, and, if you can, a small file you made yourself that triggers it. You'll get an answer within a few days.

## What's in scope

- A crafted file that crashes Swirvo, hangs it, makes it write outside the output folder, or change or delete the original.
- Anything that makes Swirvo send data over the network (it never should).
- Problems in the installer, uninstaller or the worker sandbox.

## Supported versions

Only the latest release gets security fixes.
