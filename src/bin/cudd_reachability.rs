#![allow(unused, non_snake_case)]

use biodivine_lib_param_bn::{BinaryOp, BooleanNetwork, FnUpdate, VariableId};
use cudd_sys::cudd::{
    Cudd_DagSize, Cudd_Deref, Cudd_DisableGarbageCollection, Cudd_DisableReorderingReporting,
    Cudd_Init, Cudd_ReadLogicZero, Cudd_ReadOne, Cudd_ReadZero, Cudd_Ref, Cudd_bddAnd,
    Cudd_bddExistAbstract, Cudd_bddIte, Cudd_bddIthVar, Cudd_bddLeq, Cudd_bddNand, Cudd_bddOr,
    Cudd_bddXnor, Cudd_bddXor,
};
use cudd_sys::{DdManager, DdNode};
use std::convert::TryFrom;
use std::io::Read;
use std::os::raw::c_int;
use std::time::SystemTime;

fn main() {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer).unwrap();
    let model = BooleanNetwork::try_from(buffer.as_str()).unwrap();

    let cudd: *mut DdManager = unsafe { Cudd_Init(0, 0, 1_000_000, 1_000_000, 0) };
    unsafe {
        Cudd_DisableGarbageCollection(cudd);
    }
    //unsafe { Cudd_DisableGarbageCollection(cudd); }

    let cudd_variables: Vec<*mut DdNode> = model.variables().map(|v| {
        let id = usize::from(v);
        let v = unsafe { Cudd_bddIthVar(cudd, id as c_int) };
        unsafe { Cudd_Ref(v) };
        v
    }).collect();

    let cudd_variables: Vec<*mut DdNode> = model
        .variables()
        .map(|v| {
            let id = usize::from(v);
            unsafe { Cudd_bddIthVar(cudd, id as c_int) }
        })
        .collect();

    let update_functions: Vec<*mut DdNode> = model
        .variables()
        .map(|v| {
            let update = model.get_update_function(v).as_ref().unwrap();
            fn_update_to_cudd(cudd, update)
        })
        .collect();

    let start = SystemTime::now();

    let mut universe = unsafe { Cudd_ReadOne(cudd) };
    unsafe { Cudd_Ref(universe) };
    while universe != unsafe { Cudd_ReadLogicZero(cudd) } {
        println!("Universe size: {}", unsafe { Cudd_DagSize(universe) });

        let mut i = 0;
        let mut reachability = pick_a_vertex(cudd, &cudd_variables, universe);
        unsafe { Cudd_Ref(reachability) };
        loop {
            i += 1;
            let mut done = true;

            for i_v in 0..cudd_variables.len() {
                let successors = successors(cudd, reachability, cudd_variables[i_v], update_functions[i_v]);
                unsafe { Cudd_Ref(successors) };
                let successors2 = unsafe { Cudd_bddAndNot(cudd, successors, reachability) };
                unsafe { Cudd_Ref(successors2) };
                unsafe { Cudd_Deref(successors) };
                let successors3 = unsafe { Cudd_bddAnd(cudd, successors2, universe) };
                unsafe { Cudd_Ref(successors3) };
                unsafe { Cudd_Deref(successors2) };

                //println!("Successors: {}", unsafe { Cudd_DagSize(successors) });

                if successors3 != unsafe { Cudd_ReadLogicZero(cudd) } {
                    done = false;
                    let updated = unsafe { Cudd_bddOr(cudd, successors3, reachability) };
                    unsafe { Cudd_Ref(updated) };
                    unsafe { Cudd_Deref(reachability) };
                    reachability = updated;
                    let elapsed = start.elapsed().unwrap().as_millis();
                    println!("({}) Iteration ({}), reach size: {}", elapsed, i, unsafe { Cudd_DagSize(reachability) });
                    unsafe { Cudd_Deref(successors3) };
                }
            }

            println!("Iteration ({}), reach size: {}", i, unsafe {
                Cudd_DagSize(reachability)
            });

            if done {
                break;
            }
        }

        let updated = unsafe { Cudd_bddAndNot(cudd, universe, reachability) };
        unsafe { Cudd_Ref(updated) };
        unsafe { Cudd_Deref(universe) };
        universe = updated;

        unsafe { Cudd_Deref(reachability) };
    }
}

fn successors(
    cudd: *mut DdManager,
    set: *mut DdNode,
    variable: *mut DdNode,
    update: *mut DdNode,
) -> *mut DdNode {
    let states_with_v = unsafe { Cudd_bddAnd(cudd, set, variable) };
    unsafe { Cudd_Ref(states_with_v) };
    let states_with_not_v = unsafe { Cudd_bddAndNot(cudd, set, variable) };
    unsafe { Cudd_Ref(states_with_not_v) };

    let go_up = unsafe { Cudd_bddAnd(cudd, states_with_not_v, update) };
    unsafe { Cudd_Ref(go_up) };
    //unsafe { Cudd_Deref(states_with_v) };
    let go_down = unsafe { Cudd_bddAndNot(cudd, states_with_v, update) };
    unsafe { Cudd_Ref(go_down) };
    //unsafe { Cudd_Deref(states_with_not_v) };

    let go_up2 = unsafe { Cudd_bddExistAbstract(cudd, go_up, variable) };
    unsafe { Cudd_Ref(go_up2) };
    //unsafe { Cudd_Deref(go_up) };
    let go_down2 = unsafe { Cudd_bddExistAbstract(cudd, go_down, variable) };
    unsafe { Cudd_Ref(go_down2) };
    //unsafe { Cudd_Deref(go_down) };

    let went_up = unsafe { Cudd_bddAnd(cudd, go_up2, variable) };
    unsafe { Cudd_Ref(went_up) };
    //unsafe { Cudd_Deref(go_up2) };
    let went_down = unsafe { Cudd_bddAndNot(cudd, go_down2, variable) };
    unsafe { Cudd_Ref(went_down) };
    //unsafe { Cudd_Deref(go_down2) };

    let result = unsafe { Cudd_bddOr(cudd, went_up, went_down) };
    //unsafe { Cudd_Deref(went_up) };
   // unsafe { Cudd_Deref(went_down) };
    result
}

unsafe fn Cudd_bddAndNot(
    cudd: *mut DdManager,
    left: *mut DdNode,
    right: *mut DdNode,
) -> *mut DdNode {
    Cudd_bddIte(cudd, right, Cudd_ReadLogicZero(cudd), left)
}

fn pick_a_vertex(
    cudd: *mut DdManager,
    variables: &Vec<*mut DdNode>,
    set: *mut DdNode,
) -> *mut DdNode {
    let mut candidates = set;
    unsafe { Cudd_Ref(candidates) };
    for v in variables {
        let mut next = unsafe { Cudd_bddAndNot(cudd, candidates, *v) };
        unsafe { Cudd_Ref(next) };
        if next == unsafe { Cudd_ReadLogicZero(cudd) } {
            next = unsafe { Cudd_bddAnd(cudd, candidates, *v) };
            unsafe { Cudd_Ref(next) };
        }
        //unsafe { Cudd_Deref(candidates) };
        candidates = next;
    }

    candidates
}

fn fn_update_to_cudd(cudd: *mut DdManager, update: &FnUpdate) -> *mut DdNode {
    let result = match update {
        FnUpdate::Const(value) => {
            if *value {
                unsafe { Cudd_ReadOne(cudd) }
            } else {
                unsafe { Cudd_ReadLogicZero(cudd) }
            }
        }
        FnUpdate::Param(_, _) => {
            panic!("Parametrised functions not supported.")
        }
        FnUpdate::Var(id) => {
            let id = usize::from(*id);
            unsafe { Cudd_bddIthVar(cudd, id as i32) }
        }
        FnUpdate::Not(update) => {
            // This is a little awkward because negation is not in C-bindings
            let result = fn_update_to_cudd(cudd, update);
            unsafe { Cudd_bddNand(cudd, result, result) }
        }
        FnUpdate::Binary(op, left, right) => {
            let left = fn_update_to_cudd(cudd, left);
            let right = fn_update_to_cudd(cudd, right);
            unsafe {
                match op {
                    BinaryOp::And => Cudd_bddAnd(cudd, left, right),
                    BinaryOp::Or => Cudd_bddOr(cudd, left, right),
                    BinaryOp::Iff => Cudd_bddXnor(cudd, left, right),
                    BinaryOp::Xor => Cudd_bddXor(cudd, left, right),
                    BinaryOp::Imp => Cudd_bddIte(cudd, left, right, Cudd_ReadOne(cudd)),
                }
            }
        }
    };
    unsafe { Cudd_Ref(result) };

    result
}
