# Introduction

> Note: This edition of the book is the same as [The Rust Programming
> Language][nsprust] available in print and ebook format from [No Starch
> Press][nsp], but adapted for RHDL (Rust Hardware Description Language).

[nsprust]: https://nostarch.com/rust-programming-language-2nd-edition
[nsp]: https://nostarch.com/

Welcome to *The little RHDLer*, an introductory book about RHDL. The RHDL
programming language helps you write digital hardware that is fast, safe, and
synthesizable. RHDL's high-level ergonomics and low-level control make it a
great language for digital design, from simple combinational logic to complex
processors and digital signal processing systems.

This book is written for readers who already know how to program in at least
one programming language, ideally Rust. We don't assume any prior experience
with hardware design or digital circuits, but we do assume basic familiarity
with computer science concepts. If you're completely new to programming, you
would be better served by reading a book that specifically provides an
introduction to programming, such as [*The Rust Programming Language*][rust-book].

[rust-book]: https://doc.rust-lang.org/book/

## Who RHDL Is For

RHDL is ideal for many people for a variety of reasons. Let's look at a few of
the most important groups.

### Teams of Hardware Developers

RHDL is proving to be a productive tool for collaborating among large teams of
hardware developers with varying levels of digital design experience. 
Experienced hardware engineers can leverage their knowledge while newcomers
can learn hardware design through Rust's familiar syntax and modern tooling.

RHDL brings contemporary developer tooling to the hardware development world:

* Cargo, the included dependency manager and build tool, makes adding,
  compiling, and managing dependencies painless and consistent across the RHDL
  ecosystem.
* The RHDL simulator provides fast iteration cycles and comprehensive testing
  capabilities.
* Rustfmt ensures a consistent coding style across developers.
* The Rust Language Server powers Integrated Development Environment (IDE)
  integration for code completion and inline error messages.

By using these and other tools in the RHDL ecosystem, developers can be
productive while writing hardware code.

### Students

RHDL is for students and those who are interested in learning about digital
design concepts. Using RHDL, many people have learned about topics like
finite state machines, pipelining, and parallel processing. This book can
serve as a resource for those learning paths.

### Companies

Hundreds of companies, large and small, use RHDL in production for a variety
of digital design tasks, including rapid prototyping, FPGA deployment, and
ASIC development. These companies range from startups to large corporations,
and their use cases range from simple glue logic to complex processors.

### Researchers

RHDL's emphasis on safety and performance makes it suitable for research in
computer architecture, digital signal processing, and parallel computing.
Many academic papers have used RHDL for implementing and evaluating new
hardware architectures.

### People Who Value Speed and Stability

RHDL strives for both speed and stability, meaning you don't have to choose
between fast simulation and reliable synthesis. RHDL's type system helps
prevent entire classes of hardware bugs at compile time, particularly those
related to bit widths, timing, and resource conflicts.

When designs do have bugs, RHDL's comprehensive testing and simulation
environment makes them easier to track down and fix.

## Who This Book Is For

This book assumes that you've written code in another programming language but
doesn't make any assumptions about which one. We've tried to make the material
broadly accessible to those from a wide variety of programming backgrounds.
We don't spend a lot of time talking about what programming *is* or how to
think about it. If you're entirely new to programming, you would be better
served by reading a book that specifically provides an introduction to
programming.

## How to Use This Book

In general, this book assumes that you're reading it in sequence from front to
back. Later chapters build on concepts in earlier chapters, and earlier
chapters might not delve into details on a topic but will revisit the topic
in a later chapter.

You'll find two kinds of chapters in this book: concept chapters and project
chapters. In concept chapters, you'll learn about an aspect of RHDL. In
project chapters, we'll build small programs together, applying what you've
learned so far. Chapters 2, 5, and 7 are project chapters; the rest are
concept chapters.

Chapter 1 explains how to install RHDL, how to write a "Hello, world!" program,
and how to use Cargo, RHDL's package manager and build tool. Chapter 2 is a
hands-on introduction to writing a program in RHDL, having you build up a
counter circuit. Here, we cover concepts at a high level, and later chapters
will provide additional detail. If you want to get your hands dirty right
away, Chapter 2 is the place for that. Chapter 3 covers RHDL features that
are similar to those in other programming languages, and Chapter 4 discusses
RHDL's ownership system. If you're particularly meticulous learner who prefers
to learn every detail before moving on to the next, you might want to skip
Chapter 2 and go straight to Chapter 3, returning to Chapter 2 when you'd
like to work on a project applying the details you've learned.

Chapters 5 through 7 introduce concepts central to working with RHDL in
hardware projects. Chapter 5 discusses testing and simulation, Chapter 6
introduces advanced language features, and Chapter 7 covers FPGA deployment.
After reading these chapters, you'll have the knowledge to write robust RHDL
programs.

The rest of the book covers advanced topics. Chapter 8 discusses when you
might not want to use RHDL and when you might want to interface with other
tools. Chapters 9 and 10 dive deep into RHDL internals and advanced patterns.
You probably won't need to refer to these chapters very often, but they're
available when you want to understand edge cases or optimize for the absolute
best performance.

Finally, some appendixes contain useful information about the language in a
more reference format. Appendix A covers RHDL's keywords, Appendix B covers
RHDL's operators and symbols, Appendix C covers derivable traits provided by
the standard library, Appendix D covers some useful development tools, and
Appendix E explains RHDL editions. In Appendix F, you'll find translations of
the book, and in Appendix G we cover how RHDL is made and what nightly RHDL is.

There's no wrong way to read this book: if you want to skip ahead, go for it!
You might have to jump back to earlier chapters if you experience any
confusion. But do whatever works for you.

<span id="ferris"></span>

An important part of the process of learning RHDL is learning to read the
error messages the compiler displays: these will guide you toward working
code. As such, we'll provide many examples that don't compile along with the
error message the compiler will show you in each situation. Know that if you
enter and run a random example, it may not compile! Make sure you read the
surrounding text to see whether the example you're trying to run is meant to
error. Ferris will also help you distinguish code that isn't meant to work:

| Ferris                                                                                                           | Meaning                                          |
|------------------------------------------------------------------------------------------------------------------|--------------------------------------------------|
| <img src="img/ferris/does_not_compile.svg" class="ferris-explain" alt="Ferris with a question mark"/>            | This code does not compile!                     |
| <img src="img/ferris/panics.svg" class="ferris-explain" alt="Ferris throwing up their hands"/>                   | This code panics!                               |
| <img src="img/ferris/not_desired_behavior.svg" class="ferris-explain" alt="Ferris with one claw up, shrugging"/> | This code does not produce the desired behavior |

In most situations, we'll lead you to the correct version of any code that
doesn't compile.

## Source Code

The source files from which this book is generated can be found on
[GitHub][book].

[book]: https://github.com/rhdl-org/book