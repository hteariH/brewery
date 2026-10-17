# Security policy

## Supported versions

Security fixes go into the latest Brewery release. Please update to it before reporting.

## Reporting a vulnerability

Please **don't** open a public issue for a security problem. Report it privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability**.

Include what the problem is, how to reproduce it, which version and operating system you use, and what an attacker could do with it. You'll get an answer as soon as possible; once a fix is released, the report can be made public with credit to you if you wish.

Areas that matter most: the auto-updater (downloads and runs installers on Windows), database passwords and the OS secret store, and anything that runs commands (Git, Gradle/Maven, the daemon).
