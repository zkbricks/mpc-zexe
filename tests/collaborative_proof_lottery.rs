use ark_std::*;
use std::ops::*;
use ark_poly::{
    Polynomial,
    univariate::DensePolynomial
};

use lib_mpc_zexe::utils;
use lib_mpc_zexe::collaborative_snark::PlonkProof;
use lib_mpc_zexe::coin::*;

type F = ark_bls12_377::Fr;

pub fn prover<const N: usize>(
    input_coins_poly: &[DensePolynomial<F>],
    output_coins_poly: &[DensePolynomial<F>],
) -> (Vec<DensePolynomial<F>>, Vec<DensePolynomial<F>>) {
    let lagrange_polynomials = (0..N)
        .map(|i| utils::lagrange_poly(N, i))
        .collect::<Vec<DensePolynomial<F>>>();

    // conservation: input[0].amount + input[1].amount = output[0].amount
    let lhs_poly_1 = lagrange_polynomials[AMOUNT].clone()
        .mul(
            &(input_coins_poly[0].clone()
            .add(input_coins_poly[1].clone())
            .sub(&output_coins_poly[0]))
        );

    // same asset id: input[0].asset_id = output[0].asset_id
    let lhs_poly_2 = lagrange_polynomials[ASSET_ID].clone()
        .mul(
            &(input_coins_poly[0].clone()
            .sub(&output_coins_poly[0]))
        );

    // same asset id: input[1].asset_id = output[0].asset_id
    let lhs_poly_3 = lagrange_polynomials[ASSET_ID].clone()
        .mul(
            &(input_coins_poly[1].clone()
            .sub(&output_coins_poly[0]))
        );

    // same asset id: input[0].app_id = output[0].asset_id
    let app_id_lottery_poly = utils::poly_eval_mult_const(
        &lagrange_polynomials[APP_ID].clone(),
        &F::from(AppId::LOTTERY as u64)
    );

    let lhs_poly_4 = lagrange_polynomials[APP_ID].clone()
        .mul(
            &input_coins_poly[0].clone()
            .sub(&app_id_lottery_poly)
        );

    let lhs_poly_5 = lagrange_polynomials[APP_ID].clone()
        .mul(
            &input_coins_poly[1].clone()
            .sub(&app_id_lottery_poly)
        );

    (vec![lhs_poly_1, lhs_poly_2, lhs_poly_3, lhs_poly_4, lhs_poly_5], vec![])
}


pub fn verifier<const N: usize>(
    r: &F, proof: &PlonkProof
) -> Vec<F> {
    let lagrange_polynomials = (0..N)
        .map(|i| utils::lagrange_poly(N, i))
        .collect::<Vec<DensePolynomial<F>>>();

    let app_id_lottery_poly = utils::poly_eval_mult_const(
        &lagrange_polynomials[APP_ID].clone(),
        &F::from(AppId::LOTTERY as u64)
    );

    // polynomial identity with Schwartz-Zippel
    let lhs_1 = lagrange_polynomials[AMOUNT].evaluate(&r) * 
        (
            proof.input_coins_opening[0] +
            proof.input_coins_opening[1] -
            proof.output_coins_opening[0]
        );

    let lhs_2 = lagrange_polynomials[ASSET_ID].evaluate(&r) * 
        (
            proof.input_coins_opening[0] -
            proof.output_coins_opening[0]
        );

    let lhs_3 = lagrange_polynomials[ASSET_ID].evaluate(&r) * (
        proof.input_coins_opening[1] -
        proof.output_coins_opening[0]
    );

    let lhs_4 = lagrange_polynomials[APP_ID].evaluate(&r) *
        (   
            proof.input_coins_opening[0] -
            app_id_lottery_poly.evaluate(&r)
        );

    let lhs_5 = lagrange_polynomials[APP_ID].evaluate(&r) *
    (   
        proof.input_coins_opening[1] -
        app_id_lottery_poly.evaluate(&r)
    );

    vec![lhs_1, lhs_2, lhs_3, lhs_4, lhs_5]
}

#[cfg(test)]
mod tests {
    use lib_mpc_zexe::record_commitment::kzg::*;
    use rand_chacha::rand_core::SeedableRng;
    use rand::RngCore;
    use lib_mpc_zexe::collaborative_snark::plonk::*;

    use super::*;

    #[test]
    fn test_plonk_lottery() {
        let seed = [0u8; 32];
        let mut rng = rand_chacha::ChaCha8Rng::from_seed(seed);

        let crs = JZKZGCommitmentParams::<8>::trusted_setup(&mut rng);

        let mut entropy = [0u8; 24];
        rng.fill_bytes(&mut entropy);

        let mut blind = [0u8; 24];
        rng.fill_bytes(&mut blind);

        // some values s.t. [2] = [0] + [1]
        let coin_amounts = [15u8, 22u8, 37u8];

        let mut coins = Vec::new();
        for i in 0..3 {
            let fields: [Vec<u8>; 8] = 
            [
                entropy.to_vec(),
                vec![0u8], //owner
                vec![1u8], //asset id
                vec![coin_amounts[i]], //amount
                vec![AppId::LOTTERY as u8], //app id
                vec![0u8],
                vec![0u8],
                vec![0u8],
            ];

            let coin = JZRecord::<8>::new(&crs, &fields, &blind.to_vec());
            coins.push(coin.fields());
        }

        let proof = plonk_prove(
            &crs, 
            vec![coins[0].clone(), coins[1].clone()].as_slice(), 
            vec![coins[2].clone()].as_slice(),
            super::prover::<8>
        );

        plonk_verify(
            &crs,
            &proof,
            super::verifier::<8>
        );
        
    }

}