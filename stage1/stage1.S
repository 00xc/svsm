/* SPDX-License-Identifier: MIT OR Apache-2.0 */

/*
 * Copyright (c) 2022-2023 SUSE LLC
 *
 * Author: Joerg Roedel <jroedel@suse.de>
 */

#include "types.h"

/* Use first 640kb of memory for stage2 loader */
#define STAGE2_RANGE_START	0
#define STAGE2_RANGE_PAGES	158

/* Stage2 is loaded at 64kb */
#define STAGE2_START		0x10000

#define STAGE1_STACK		0x10000

.text
	.section ".startup.text","ax"
	.code32

	.org 0
	.globl startup_32
startup_32:
	cld
	cli

	/* Enable caches */
	movl	%cr0, %eax
	andl	$~((1 << 30) | (1 << 29)), %eax
	mov	%eax, %cr0

#ifdef LOAD_STAGE2
	/* Setup Stack */
	movl	$STAGE1_STACK, %esp

	/* Store run-offset to %ebp */
	call	2f
2:	popl	%ebp
	leal	2b, %eax
	subl	%eax, %ebp

	leal	stage2_bin(%ebp), %esi
	movl	$STAGE2_START, %edi
	movl	stage2_size(%ebp), %ecx
	shrl	$2, %ecx
	rep movsl

	/* Setup stack for stage 2*/
	movl	$STAGE2_START, %esp

	/* Write startup information to stage2 stack */
	xorl	%eax, %eax
	pushl	%eax

	leal	kernel_fs_bin_end(%ebp), %edi
	pushl	%edi

	leal	kernel_fs_bin(%ebp), %edi
	pushl	%edi

	leal	kernel_elf_end(%ebp), %edi
	pushl	%edi

	leal	kernel_elf(%ebp), %edi
	pushl	%edi

	/* Push the value 1 to indicate SNP */
	pushl	$1

	/* Reserve space for VTOM */
	pushl	%eax
	pushl	%eax
#else
	/*
	 * Stage 2 launch info has been prepared
	 * Make sure platform type is TDP
	 */
	movl	$(STAGE2_START - 24), %eax
	movl	(%eax), %eax
	cmpl	$2, %eax
	je	.Lsetup_td
	ud2

.Lsetup_td:
	/* %esi is initialized with TD CPU index */
	test	%esi, %esi
	jz	.Lsetup_bsp_stack

	/* Set up invalid stack for APs since they must run stacklessly */
	movl	$0x7ffff000, %esp
	jmp	.Lenter_stage2

.Lsetup_bsp_stack:
	/* Set up BSP stack for stage 2 */
	movl	$(STAGE2_START - 32), %esp
	/* %ebx is initialized with GPAW - save (1u64 << (GPAW - 1)) to vtom */
	mov	%esp, %eax
	/* GPAW must be either 48 or 52 */
	xorl	%ecx, %ecx
	movl	%ecx, (%eax)
	addl	$4, %eax
	subl	$33, %ebx
	bts	%ebx, %ecx
	movl	%ecx, (%eax)
.Lenter_stage2:
#endif

	/* Jump to stage 2 */
	movl	$STAGE2_START, %eax
	jmp	*%eax

.data

#ifdef LOAD_STAGE2
	.align	4
stage2_bin:
	.incbin "bin/stage2.bin"
	.align 4
stage2_bin_end:

kernel_elf:
	.incbin "bin/kernel.elf"
	.align 4
kernel_elf_end:

kernel_fs_bin:
	.incbin "bin/svsm-fs.bin"
kernel_fs_bin_end:

	.align 4
stage2_size:
	.long	stage2_bin_end - stage2_bin
#endif
