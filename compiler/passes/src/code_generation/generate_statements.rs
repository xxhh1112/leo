// Copyright (C) 2019-2023 Aleo Systems Inc.
// This file is part of the Leo library.

// The Leo library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The Leo library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the Leo library. If not, see <https://www.gnu.org/licenses/>.

use crate::CodeGenerator;

use leo_ast::{
    AssertStatement, AssertVariant, AssignStatement, Block, ConditionalStatement, ConsoleStatement, DecrementStatement,
    DefinitionStatement, Expression, ExpressionConsumer, ExpressionStatement, IncrementStatement, IterationStatement,
    Mode, Output, ReturnStatement, StatementConsumer,
};

use itertools::Itertools;
use std::fmt::Write as _;

impl StatementConsumer for CodeGenerator<'_> {
    type Output = String;

    fn consume_assert(&mut self, input: AssertStatement) -> Self::Output {
        let mut generate_assert_instruction = |name: &str, left: Expression, right: Expression| {
            let (left_operand, left_instructions) = self.consume_expression(left);
            let (right_operand, right_instructions) = self.consume_expression(right);
            let assert_instruction = format!("    {name} {left_operand} {right_operand};\n");

            // Concatenate the instructions.
            let mut instructions = left_instructions;
            instructions.push_str(&right_instructions);
            instructions.push_str(&assert_instruction);

            instructions
        };
        match input.variant {
            AssertVariant::Assert(expr) => {
                let (operand, mut instructions) = self.consume_expression(expr);
                let assert_instruction = format!("    assert.eq {operand} true;\n");

                instructions.push_str(&assert_instruction);
                instructions
            }
            AssertVariant::AssertEq(left, right) => generate_assert_instruction("assert.eq", left, right),
            AssertVariant::AssertNeq(left, right) => generate_assert_instruction("assert.neq", left, right),
        }
    }

    fn consume_assign(&mut self, input: AssignStatement) -> Self::Output {
        match (&input.place, &input.value) {
            (Expression::Identifier(identifier), _) => {
                let (operand, expression_instructions) = self.consume_expression(input.value);
                self.variable_mapping.insert(identifier.name, operand);
                expression_instructions
            }
            (Expression::Tuple(tuple), Expression::Call(_)) => {
                let (operand, expression_instructions) = self.consume_expression(input.value);
                // Split out the destinations from the tuple.
                let operands = operand.split(' ').collect::<Vec<_>>();
                // Add the destinations to the variable mapping.
                tuple.elements.iter().zip_eq(operands).for_each(|(element, operand)| {
                    match element {
                        Expression::Identifier(identifier) => {
                            self.variable_mapping.insert(identifier.name, operand.to_string())
                        }
                        _ => {
                            unreachable!("Type checking ensures that tuple elements on the lhs are always identifiers.")
                        }
                    };
                });
                expression_instructions
            }
            _ => unimplemented!(
                "Code generation for the left-hand side of an assignment is only implemented for `Identifier`s."
            ),
        }
    }

    fn consume_block(&mut self, input: Block) -> Self::Output {
        // For each statement in the block, consume it and add its instructions to the list.
        input
            .statements
            .into_iter()
            .map(|stmt| self.consume_statement(stmt))
            .join("")
    }

    fn consume_conditional(&mut self, _input: ConditionalStatement) -> Self::Output {
        // TODO: Once SSA is made optional, create a Leo error informing the user to enable the SSA pass.
        unreachable!("`ConditionalStatement`s should not be in the AST at this phase of compilation.")
    }

    fn consume_console(&mut self, _: ConsoleStatement) -> Self::Output {
        unreachable!("Parsing guarantees that `ConsoleStatement`s are not present in the AST.")
    }

    fn consume_decrement(&mut self, input: DecrementStatement) -> Self::Output {
        let (index, mut instructions) = self.consume_expression(input.index);
        let (amount, amount_instructions) = self.consume_expression(input.amount);
        instructions.push_str(&amount_instructions);
        instructions.push_str(&format!("    decrement {}[{index}] by {amount};\n", input.mapping));

        instructions
    }

    fn consume_definition(&mut self, _input: DefinitionStatement) -> Self::Output {
        // TODO: If SSA is made optional, then conditionally enable codegen for DefinitionStatement
        unreachable!("DefinitionStatement's should not exist in SSA form.")
    }

    fn consume_expression_statement(&mut self, input: ExpressionStatement) -> Self::Output {
        match &input.expression {
            Expression::Call(_) => {
                // Note that codegen for CallExpression in an expression statement does not return any destination registers.
                self.consume_expression(input.expression).1
            }
            _ => unreachable!("ExpressionStatement's can only contain CallExpression's."),
        }
    }

    fn consume_increment(&mut self, input: IncrementStatement) -> Self::Output {
        let (index, mut instructions) = self.consume_expression(input.index);
        let (amount, amount_instructions) = self.consume_expression(input.amount);
        instructions.push_str(&amount_instructions);
        instructions.push_str(&format!("    increment {}[{index}] by {amount};\n", input.mapping));

        instructions
    }

    fn consume_iteration(&mut self, _input: IterationStatement) -> Self::Output {
        // TODO: Once loop unrolling is made optional, create a Leo error informing the user to enable the loop unrolling pass..
        unreachable!("`IterationStatement`s should not be in the AST at this phase of compilation.");
    }

    fn consume_return(&mut self, input: ReturnStatement) -> Self::Output {
        let mut instructions = match input.expression {
            // Skip empty return statements.
            Expression::Unit(_) => String::new(),
            expression => {
                let (operand, mut expression_instructions) = self.consume_expression(expression);
                // Note that the first unwrap is safe, since `current_function` is set in `consume_function`.
                // Note that the second unwrap is safe, since type checking guarantees that the function is defined.
                let function_symbol = self
                    .symbol_table
                    .lookup_fn_symbol(self.current_function.unwrap())
                    .unwrap();
                // Get the output type of the function.
                let output = if self.in_finalize {
                    // Note that this unwrap is safe since symbol table construction will store information about the finalize block if it exists.
                    function_symbol.finalize.as_ref().unwrap().output.iter()
                } else {
                    function_symbol.output.iter()
                };
                let instructions = operand
                    .split(' ')
                    .zip_eq(output)
                    .map(|(operand, output)| {
                        match output {
                            Output::Internal(output) => {
                                let visibility = if self.is_transition_function {
                                    match self.in_finalize {
                                        // If in finalize block, the default visibility is public.
                                        true => match output.mode {
                                            Mode::None => Mode::Public,
                                            mode => mode,
                                        },
                                        // If not in finalize block, the default visibility is private.
                                        false => match output.mode {
                                            Mode::None => Mode::Private,
                                            mode => mode,
                                        },
                                    }
                                } else {
                                    // Only program functions have visibilities associated with their outputs.
                                    Mode::None
                                };
                                format!(
                                    "    output {} as {};\n",
                                    operand,
                                    self.consume_type_with_visibility(&output.type_, visibility)
                                )
                            }
                            Output::External(output) => {
                                format!(
                                    "    output {} as {}.aleo/{}.record;\n",
                                    operand, output.program_name, output.record,
                                )
                            }
                        }
                    })
                    .join("");

                expression_instructions.push_str(&instructions);

                expression_instructions
            }
        };

        // Output a finalize instruction if needed.
        // TODO: Check formatting.
        if let Some(arguments) = input.finalize_arguments {
            let mut finalize_instruction = "\n    finalize".to_string();

            for argument in arguments.into_iter() {
                let (argument, argument_instructions) = self.consume_expression(argument);
                write!(finalize_instruction, " {argument}").expect("failed to write to string");
                instructions.push_str(&argument_instructions);
            }
            writeln!(finalize_instruction, ";").expect("failed to write to string");

            instructions.push_str(&finalize_instruction);
        }

        instructions
    }
}